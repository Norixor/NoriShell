# Plugin-host bridge boundary

Application IPC manages package lifecycle, protected dialogs, and host-rendered projections. The isolated guest speaks protocol 1.13 to Plugin Host over the SDK ABI. Core injects package identity, immutable package hash, grants, owner, scope, and generation; it rejects guest-supplied equivalents.

Use `plugin_*` Core commands only from the NoriShell renderer or the specifically ACL-scoped secure surface that owns that screen. A plugin author uses SDK `api.request`, opaque handles, and Core-provided callbacks. `api.request` is admitted again at execution time; it never becomes permission to invoke Tauri IPC.

This separation prevents a Plugin page, Wasm guest, declarative UI, isolated WebView, or host-DOM contribution from reaching Vault, SSH internals, internal Core commands, SQLite, a raw transport/channel, or a secure prompt. Background `onOpen`, timer, restart-recovery, and provider paths must return a typed non-interactive result such as `interactionRequired` or a Vault state; they cannot create, unlock, or recover Vault interaction themselves.
