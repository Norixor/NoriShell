# 应用 IPC command 目录

这是应用 IPC 表面的维护索引。每个稳定 command 精确的 `RequestType → ResultType` 以 [`core-api.ts`](../../../src/core-api/generated/core-api.ts) 为准。完整稳定清单见[生成 command 目录](commands.generated.md)：它来自 `COMMAND_*` 声明，共 210 项。实际 production registry 另见[handler 与 ACL 目录](handlers.generated.md)：它来自 `production_invoke_handler!` 与受信任主窗口 allowlist，共 277 个 handler。

| 分组 | Commands |
|---|---|
| Hosts 与 identities | `host_*`、`identity_*`、`credential_*`、`ssh_agent_*`、`openssh_config_*`、`known_host_*` |
| Policies 与导入 | `recent_connection_list`、`route_plan_replace`、`authentication_plan_replace`、`algorithm_policy_*`、`heartbeat_policy_replace`、`monitoring_policy_replace`、`login_automation_*` |
| Plugin lifecycle 与 secure bridge | `plugin_*`，包括 app integration、workflow、protocol launch、isolated bridge 与 secure prompt command |
| Overview 与 Metrics | `ssh_connection_test`、`server_overview_snapshot`、`metrics_*` |
| 远程桌面与通知 | `desktop_*` profile、availability、open/session、prompt、focus/input、frame、`desktop_audio_mute`，以及 native notification command |
| SSH、local、Telnet terminal | `ssh_terminal_*`、`terminal_input_focus_*`、`local_terminal_*`、`telnet_terminal_*` |
| Port forward 与 SFTP | `forward_*`、`sftp_*` |
| Workspace、Vault、window、exit | `terminal_workspace_*`、`vault_*`、`window_*`、`application_request_exit` |

注册不等于权限：主 WebView 和每个 secure surface 都有单独的 Tauri ACL，生成目录已列出。`desktop_audio_mute` 是已注册的 Wry handler，使用类型化 audio state/mute request；它出现在目录中不能证明 native audio 已验收。此 IPC 永远不是插件 API；插件的同名动作必须通过类型化 broker request 表达，并由 Core 单独授权，guest 不能按 handler 名调用它。


`desktop_preferences_get` / `desktop_preferences_replace` 只供可信主窗口读取与 CAS 替换非秘密桌面偏好；revision 使用十进制字符串。`tray_actions_ready` 在注册 `native-tray-action` listener 后调用，返回待消费 token；`tray_action_take` 消费绑定原资源代次的一次性 token，不接受 renderer 提交目标。新建动作仍经过原 Launcher/认证门禁；资源定位绝不重连。

`ssh_sync_preferences_publish`、`ssh_sync_preferences_pending_get`、`ssh_sync_preferences_apply_ack`、`ssh_sync_preferences_retry_pending` 仅供可信主窗口提交经校验的非秘密偏好快照、读取恢复待办与按组确认结果。它们不属于稳定 `COMMAND_*` 接口，也不向插件开放；Core 持久化待办并核对恢复代次后才允许应用。

`native_json_export` 是受信任主窗口专用的原生 JSON 保存命令（`preferences | shortcuts`、JSON 文本 → 是否保存），不属于插件 API。目标路径只能由本次系统保存对话框产生；取消不写入，失败不回显路径，不授予通用文件写权限。

托盘面板专用 `tray_panel_snapshot/execute/hide` 仅允许 `tray-panel` 窗口：读取 `NativeTrayPanelSnapshot`，消费不透明一次性token，或收起窗口。面板不能调用主窗口Core命令；插件不能调用这三项。主窗口继续通过 `tray_actions_ready/tray_action_take` 接收显式动作并复验资源。

`NativeTrayPanelSnapshot`提供结构化stats（未知为null、隐藏时为空）、行role和notificationState；渲染器不通过解析本地化文案识别动作或资源分类。


`plugin_theme_list`: `PluginThemeListRequest → PluginThemeListResponse`; main-window only. Returns verified installed data themes, including disabled entries. It is not callable by plugins.
