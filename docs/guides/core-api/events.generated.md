# Generated Core event directory

Generated from the `EVENT_*` constants in `crates/core-api/src/lib.rs` and the exported TypeScript declarations whose names denote event or output payloads. Other Tauri event names are emitted directly from `src-tauri` without a central definition and are not listed here. Event payloads remain generation-scoped projections, not permissions.

## Named Core event symbols (2)

| Symbol | Event |
| --- | --- |
| `EVENT_PLUGIN_SETTINGS_CHANGED` | `plugin-settings-changed` |
| `EVENT_PLUGIN_SSH_SYNC_BROWSER_INVALIDATED` | `plugin-ssh-sync-browser-invalidated` |

## Event and output payload types (26)

- `DesktopInputEvent`
- `NativeTrayActionEvent`
- `WorkflowEvent`
- `PluginWorkflowEvent`
- `PluginApiResourceEvent`
- `PluginSerialEvent`
- `PluginTerminalGap`
- `PluginTerminalOutputItem`
- `PluginTerminalEvent`
- `PluginProtocolEvent`
- `PluginSubscriptionEvent`
- `PluginSftpEvent`
- `PluginNetworkEvent`
- `PluginRemoteExecEvent`
- `PluginHostNavigationEvent`
- `SshSessionOutputGap`
- `SshSessionOutputItem`
- `SshSessionEvent`
- `LocalSessionOutputGap`
- `LocalSessionOutputItem`
- `LocalSessionEvent`
- `TelnetSessionOutputGap`
- `TelnetSessionOutputItem`
- `TelnetSessionEvent`
- `ForwardSessionEvent`
- `SftpSessionEvent`
