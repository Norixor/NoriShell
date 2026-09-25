# Core internal API catalog

Core API is application-renderer IPC, maintained for NoriShell's own WebViews. It is **not** a plugin API or a capability grant. Plugins use only the [Plugin API](../plugin-api/README.en.md); they must not call a Tauri command, obtain a Core DTO by IPC, or rely on an internal handle.

Current source version is Core API **1.85**. The generated TypeScript contract is [`src/core-api/generated/core-api.ts`](../../../src/core-api/generated/core-api.ts); Rust public types and stable command declarations begin at [`crates/core-api/src/lib.rs`](../../../crates/core-api/src/lib.rs); production command registration is in [`src-tauri/src/lib.rs`](../../../src-tauri/src/lib.rs). This checkpoint contains 48 public Core modules, 857 emitted TypeScript declarations, 210 stable `COMMAND_*` names, and 276 registered Tauri handlers. The latter may include Wry-only or non-exported application plumbing, so the two lists are intentionally separate.

- [Module and type directory](modules.en.md)
- [Application IPC command directory](commands.en.md)
- [Core events and resource event directory](events.en.md)
- [Plugin-host bridge boundary](plugin-host.en.md)
- [Complete generated type directory](types.generated.md)
- [Stable generated command directory](commands.generated.md)
- [Registered handler and main-window ACL directory](handlers.generated.md)
- [Generated event payload directory](events.generated.md)

Exact generated request/result types are authoritative when implementation changes. The catalog records source interfaces; it does not claim platform, native-window, hardware, or Windows acceptance. Those remain in [implementation status](../../../README.en.md#installation-and-quick-start).
