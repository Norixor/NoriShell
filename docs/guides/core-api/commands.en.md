# Application IPC command directory

This is the maintained index for application IPC. Each stable command's exact `RequestType → ResultType` comes from [`core-api.ts`](../../../src/core-api/generated/core-api.ts). The complete stable list is [the generated command directory](commands.generated.md): 210 symbols from `COMMAND_*` declarations. The actual production registry is separately enumerated in [the handler and ACL directory](handlers.generated.md): 264 handlers from `production_invoke_handler!` and the trusted main-window allowlist.

| Group | Commands |
|---|---|
| Hosts and identities | `host_*`, `identity_*`, `credential_*`, `ssh_agent_*`, `openssh_config_*`, `known_host_*` |
| Policies and import | `recent_connection_list`, `route_plan_replace`, `authentication_plan_replace`, `algorithm_policy_*`, `heartbeat_policy_replace`, `monitoring_policy_replace`, `login_automation_*` |
| Plugin lifecycle and secure bridge | `plugin_*`, including app integrations, workflows, protocol launches, isolated bridges, and secure prompt commands |
| Overview and Metrics | `ssh_connection_test`, `server_overview_snapshot`, `metrics_*` |
| Remote desktop and notifications | `desktop_*` profiles, availability, open/session, prompt, focus/input, frames, `desktop_audio_mute`, plus native-notification commands |
| SSH, local, and Telnet terminals | `ssh_terminal_*`, `terminal_input_focus_*`, `local_terminal_*`, `telnet_terminal_*` |
| Port forwarding and SFTP | `forward_*`, `sftp_*` |
| Workspace, Vault, window, exit | `terminal_workspace_*`, `vault_*`, `window_*`, `application_request_exit` |

Registration is not permission: the main WebView and each secure surface have separate Tauri ACLs, recorded in the generated handler directory. `desktop_audio_mute` is a registered Wry handler and uses the typed audio state/mute request; its presence here is not evidence of completed native audio acceptance. This IPC is never a plugin surface. A same-looking plugin action must be a typed broker request and must pass Core authorization; guest code cannot invoke a handler by name.


`desktop_preferences_get` / `desktop_preferences_replace` read and CAS-replace non-secret preferences from the trusted main window; revisions are decimal strings. Call `tray_actions_ready` after registering the `native-tray-action` listener, then consume each immutable token with `tray_action_take`. The renderer cannot submit a replacement target. Explicit creation follows the existing launcher/authentication gates; focusing never reconnects.

`native_json_export` is a trusted-main-only native JSON save command (`preferences | shortcuts`, JSON text → saved boolean), not a plugin API. Only its system save dialog selects the destination. Cancellation does not write; errors do not expose paths; no general filesystem write permission is granted.

The `tray-panel` window alone may call `tray_panel_snapshot/execute/hide`: read `NativeTrayPanelSnapshot`, consume an opaque one-time token, or dismiss the panel. It cannot call main-window Core commands; plugins cannot call these three commands. The main window continues to consume explicit actions through `tray_actions_ready/tray_action_take` and revalidate resources.

`NativeTrayPanelSnapshot` includes structured stats (null for unknown counts; empty when hidden), semantic row roles, and notification state. The renderer never parses localized text to identify actions or resource groups.


`plugin_theme_list`: `PluginThemeListRequest → PluginThemeListResponse`; main-window only. Returns verified installed data themes, including disabled entries. It is not callable by plugins.
