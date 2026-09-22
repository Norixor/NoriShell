# Core events and resource events

Core events are projections of Core-owned state, never permission tokens. Renderer listeners must bind each event to the current resource/session/workspace generation and discard late events after detach, close, revoke, or route replacement. The complete source checkpoint is [events.generated.md](events.generated.md): two named event symbols and 25 emitted event/output payload types.

| Event family | Producer | Consumers |
|---|---|---|
| SSH/local/Telnet terminal output, state, attachment and input-focus changes | session actors | terminal renderer only |
| SFTP listing/preview/tail/transfer and Forward lifecycle | independent SFTP/Forward services | application renderer |
| Metrics samples, stale/error/backoff and Overview aggregation | independent Metrics scheduler | Overview renderer |
| Plugin package/readiness/operation/audit/contribution and target-context updates | plugin lifecycle and host bridge | plugins and registered extension surfaces |
| Plugin API resource events | Plugin API resource registry | the owning isolated Plugin Host via protocol reply/event handling |
| Vault/protected prompt and exit blocker changes | Core secure state | allowed secure surface/renderer |

The internal emitter names and transport shape are implementation detail. Plugins receive only the typed subscription/resource events defined in the [Plugin API](../plugin-api/broker.en.md), never renderer event channels. A Tauri listener, a package event, and an opaque resource handle do not transfer authority.

Native tray events are `native-tray-action { token }`, `native-tray-error { messageKey }`, and `native-tray-vault-changed`. Resource clicks use `native-resource-notification-click` with event/resource IDs, generation and revision only, never commands, names, paths or secrets. The main window revalidates current snapshots; stale clicks never reconnect. These are not plugin event permissions.
