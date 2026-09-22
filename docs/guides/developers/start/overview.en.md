# NoriShell plugin author guide

NoriShell plugins are Rust/Wasm guests that run outside the desktop application's Vue WebView and native process. A plugin can contribute a host-rendered panel, ask Core to perform a typed and reviewed operation, react to the typed result, and update its document. It cannot directly call Tauri, open sockets, read the Vault, access SQLite, inject the host DOM, or treat a declared capability as an automatic grant.

The current public author contract is protocol **1.13**. Build against the SDK source that belongs to the NoriShell checkout and package a constrained ZIP containing `manifest.json`, `plugin.wasm`, and any declared static assets.

## What you can build

| Goal | Start here | What Core continues to own |
| --- | --- | --- |
| A small visible tool that shows typed API information | [First visible plugin](./quickstart.en.md) and [API Demo](../examples/api-demo.en.md) | The panel target, rendering, clipboard approval, and API availability |
| A service integration | [Service Demo](../examples/service-demo.en.md) | HTTPS transport, exact endpoint review, resource lifecycle, and user approval |
| A task with timers and restart-safe status | [Workflow Demo](../examples/workflow.en.md) | Task records, timers, resource cleanup, revisions, and restart reconciliation |
| A controlled command, notification, navigation, or file picker | [App integration](../examples/app-integration.en.md) | Command registration, shortcut enablement, native picker, file handle, and scope approval |
| A separate HTML/CSS/JS surface | [Isolated UI](../examples/isolated-ui.en.md) | The isolated WebView, package asset lookup, and the constrained MessagePort |
| A terminal protocol provider | [Protocol Demo](../examples/protocol.en.md) | Network resources, terminal lifecycle, permission checks, and byte delivery |

## Reading path

| If you are trying to… | Read in this order |
| --- | --- |
| Make a first button appear and respond | [Quick start](./quickstart.en.md) → [API Demo](../examples/api-demo.en.md) → [UI documents](../development/ui.en.md) → [Packaging](../development/packaging.en.md) |
| Call a Core capability after a click | [Calling Core](../development/calling-api.en.md) → [API Demo](../examples/api-demo.en.md) → [Errors and user continuation](../development/errors.en.md) |
| Choose an implementation language or an HTML surface | [Languages and boundaries](./languages.en.md) → [Isolated UI](../examples/isolated-ui.en.md) |
| Turn a sample into your product | [Examples index](../examples/README.en.md) → its focused page → [Packaging](../development/packaging.en.md) |

`Initialize` normally contributes a document. A user click becomes `UiAction`; the guest may emit a typed `api.request`; Core then sends `BrokerResult`; the guest renders the next `ui.document`. The [API Demo](../examples/api-demo.en.md) walks through that full round trip.

Use the SDK and its typed DTOs from guest code. The Core-internal API catalogue is for application maintainers and is not a plugin API. A package check or an ABI harness proves only that local artifact boundary; import, capability approval, enable/disable behavior, and the actual user flow still need testing in a matching NoriShell desktop application.
