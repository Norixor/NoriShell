# Core 模块与类型

`norishell-core-api` 有 48 个重新导出的模块。全部 857 个 TypeScript 声明的机器生成列表见[types.generated.md](types.generated.md)，需要核对类型拼写和边界时以该目录为准。

| 模块族 | 模块 |
| --- | --- |
| Host 与连接策略 | `host`、`openssh_import`、`ids`、`command`、`error` |
| Terminal 与 workspace | `ssh_terminal`、`local_terminal`、`native_terminal`、`telnet_terminal`、`terminal_input`、`terminal_workspace` |
| 传输与监控 | `forward_session`、`sftp_session`、`server_overview` |
| Vault 与 desktop | `vault`、`desktop`、`version`；desktop 除 profile/session DTO 外还包含 `DesktopAudioState` 与 `DesktopAudioMuteRequest` |
| Plugin 包、权限与 Host bridge | `plugin`、`plugin_theme`、`plugin_api`、`plugin_approval_policy`、`plugin_app_integration`、`plugin_credential_api`、`plugin_device_api`、`plugin_dom`、`plugin_file_api`、`plugin_file_picker`、`plugin_host`、`plugin_isolated`、`plugin_network_api`、`plugin_operations`、`plugin_process_api`、`plugin_protocol`、`plugin_protocol_launch`、`plugin_remote_exec_api`、`plugin_resources`、`plugin_settings`、`plugin_sftp_api`、`plugin_special_permission`、`plugin_ssh_sync_browser`、`plugin_storage_api`、`plugin_subscription_api`、`plugin_terminal`、`plugin_terminal_session`、`plugin_ui`、`plugin_workflow` |

所有业务写 request 都携带适用的 request metadata、version、operation id 或 idempotency key。失败使用 `CoreApiError { category, code, retryStrategy }`；renderer 代码不得解析本地化错误文本。Core 的 public re-export 是应用 DTO，绝不是 guest SDK export。

`desktop_preferences` 定义窗口关闭、托盘显示与通知偏好及其 CAS 请求；`tray` 定义主窗口一次性动作 DTO。`SftpLocalDirectoryCapability.rememberablePath` 只在 Core 规范化路径可无损表示为 UTF-8 时返回，否则为 null；不得用 displayName 拼接物理路径。路径记忆不复用旧 capability，重新访问仍须重新注册与校验。
