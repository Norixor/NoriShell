# Broker 调用、资源、存储、凭据与订阅

guest 通过 `api.request` 输出类型化调用，并在协议中收到匹配的类型化回复。它不能直接调用 Tauri command、Vault、SQLite、SSH transport、SFTP subsystem 或操作系统 API。`describe` 返回当前协议、平台、可用方法和限制；不要假定一个 capability 在所有平台均可用。完整请求/结果矩阵见[插件 API Broker 方法](../plugin-api/broker.zh-CN.md)。

资源是 owner-scoped、generation-fenced 的不透明 handle。使用 `resourcesList`、`resourceEvents` 和 `resourceClose`；close 必须明确执行，取消、撤权、禁用、package replacement、过期 scope 或崩溃也可以独立关闭资源。timer 和事件订阅也是资源。订阅事件来自真实 Core 终端 metadata、已批准 Host scope 与插件设置；仅在参考明确要求轮询时才轮询。不得持久化 handle、把它当 capability，或在生命周期变化后复用。

`storage` 是统一的插件私有 JSON 存储：非秘密、有界、带 revision，并通过 CAS 保护。不要使用已废弃的 `plugin_storage`/`pluginStorage` 协议面，且绝不能把凭据放进 storage。

`networkStart`/`networkSend` 以类型化资源 broker HTTP、WSS、TCP、UDP 和 TLS。受保护操作前会冻结精确目标和已解析目的地址集合；redirect 和环境代理不能代替 grant。`filePick`/`file` 使用不透明 root，`sftpOpen`/`sftp` 使用范围化 SFTP 资源；`remoteExecStart`/`remoteExecSend` 使用独立认证且有界的远端 exec transport，`processStart`/`processSend` 使用另行批准的本地进程计划。它们均以 resource event 返回实际 open/data/exit/error/close 事实。

`credential` 为插件自有账号提供 Vault 保护、宿主渲染的输入路径。插件只得到不透明引用和非秘密状态。Core 仅会把它注入已批准的精确 HTTPS/WSS origin，不能复制进通用请求或插件 storage。

只有明确前台用户操作可触发受保护决定。`onOpen`、timer、provider callback、自动 task step、restart recovery 和 CLI harness 流量必须返回类型化非交互结果。尤其是 `vaultMissing`、`vaultLocked` 与 `vaultRequiresReload` 是交给显式用户 action 处理的 state，绝不能据此后台创建/解锁。`once` 只作用于当前 pending 的精确 operation；`always` 只有在 Core 计算出 exact operation 时才能记住，仍可撤销且受执行时 fence 约束。

受保护窗口默认“仅本次”。只有 Core 判定 `exactOperation` 时，用户才能选择“始终允许此操作”，并选择 15 分钟、1 小时、24 小时或无限期（默认）；风险说明与期限选择均由宿主渲染，guest 不得自行保存或代选。Core 保存可空的绝对过期时间而不是 duration，因此重启不能续期。网络、文件和 serial 没有通用的 permission-scope 数据库：每条 retained rule 都对冻结 request 做指纹绑定。guest 可见的 permission row 也只限当前 owner 摘要。文件和 serial 选择使用不透明管理标签；原始 canonical path 与 device identity 只停留在活动 Core scope 和受保护提示中。

授权复用与能力授予是两层检查：已有精确操作记录也不能补足缺失或已撤销的 capability。`permissions` 返回当前包的非秘密摘要与 `policyRevision`；单项 `permissionRevoke` 必须携带读取到的 revision，`permissionsForget` 清除当前 guest 拥有的操作记录。操作授权撤销不会撤销或扩大 capability。用户也可从“已安装插件 → 更多 → 操作授权”查看期限并撤销；该管理操作不需要再次批准。完整规则见[能力与受保护操作](../plugin-api/capabilities.zh-CN.md)。
