# Core modules and types

`norishell-core-api` re-exports **49 modules**. The complete machine-derived directory of **869 TypeScript declarations** is [types.generated.md](types.generated.md); use it for exact names and source modules.

| Module family | Modules |
| --- | --- |
| Host and connection policy | `host`, `openssh_import`, `ids`, `command`, `error` |
| Terminal and workspace | `ssh_terminal`, `local_terminal`, `native_terminal`, `telnet_terminal`, `terminal_input`, `terminal_workspace` |
| Transfers and monitoring | `forward_session`, `sftp_session`, `server_overview` |
| Vault and desktop | `vault`, `desktop`, `desktop_preferences`, `tray`, `version`; desktop includes `DesktopAudioState` and `DesktopAudioMuteRequest` alongside the profile/session DTOs |
| Plugin package, permissions and host bridge | `plugin`, `plugin_theme`, `plugin_api`, `plugin_data_api`, `plugin_approval_policy`, `plugin_app_integration`, `plugin_credential_api`, `plugin_device_api`, `plugin_dom`, `plugin_file_api`, `plugin_file_picker`, `plugin_host`, `plugin_isolated`, `plugin_network_api`, `plugin_operations`, `plugin_process_api`, `plugin_protocol`, `plugin_protocol_launch`, `plugin_remote_exec_api`, `plugin_resources`, `plugin_settings`, `plugin_sftp_api`, `plugin_special_permission`, `plugin_ssh_sync_browser`, `plugin_storage_api`, `plugin_subscription_api`, `plugin_terminal`, `plugin_terminal_session`, `plugin_ui`, `plugin_workflow` |

Every business write carries the applicable request metadata, version, operation id, or idempotency key. Failures use `CoreApiError { category, code, retryStrategy }`; renderer code must not parse localized error text. The public Core re-exports are application DTOs, not guest-safe SDK exports.

`desktop_preferences` defines window, tray and notification preferences and CAS requests; `tray` defines one-shot main-window action DTOs. `SftpLocalDirectoryCapability.rememberablePath` is returned only for a canonical path representable losslessly as UTF-8, otherwise null. Never build physical paths from display names. Remembered paths require fresh capability registration and validation.
