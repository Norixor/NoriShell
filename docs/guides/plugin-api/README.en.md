# Plugin API reference

- [Protocol boundary and ABI](protocol.en.md)
- [Typed broker methods, resources, events, requests, and results](broker.en.md)
- [Guest-safe request and result types](types.en.md)
- [Declarative UI, fields, and targets](ui.en.md)
- [Capabilities and protected operations](capabilities.en.md)
- [Protocol providers, catalogs, and serial/session work in progress](providers.en.md)

This is the public guest contract for protocol **1.13**. It contains 35 `PluginApiOperation` variants; `describe` supplies the runtime availability subset. Do not call any item from the [Core internal API catalog](../core-api/README.en.md) from a plugin.
