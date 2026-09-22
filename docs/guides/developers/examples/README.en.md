# Plugin examples

Each example in this section corresponds to source code maintained with NoriShell. Treat one as a focused starting point rather than an installable template for every product: change the package identity, declared capabilities, actions, state, and assets together.

| Example | Use it when you need | Local package result | What still needs desktop acceptance |
| --- | --- | --- | --- |
| [API Demo](./api-demo.en.md) | A first visible panel, typed API discovery, and copying a result | Wasm ABI verifier plus a ZIP and checksum | Import, approval, enablement, panel contribution, click result, and disable behavior |
| [Service Demo](./service-demo.en.md) | An explicitly approved HTTPS workflow | Tests, package check, ZIP, and checksum | Permission continuation, live network response, cancellation, and resource cleanup |
| [Workflow Demo](./workflow.en.md) | A timer-driven task with revisions and restart state | Wasm build and SDK package check | Task lifecycle, cancellation, and restart behavior in the application |
| [App integration](./app-integration.en.md) | Commands, optional shortcut, notification, navigation, or a native file handle | Wasm build and SDK package check | Command/shortcut, native picker, scope review, notification, and handle cleanup |
| [Isolated UI](./isolated-ui.en.md) | A packaged HTML/CSS/JS surface with a restricted bridge | Wasm build and isolated-surface ABI verification | Isolated WebView, MessagePort, approval prompts, and resource cleanup |
| [Protocol Demo](./protocol.en.md) | A framed TCP terminal provider | Loopback fixture harness, ZIP check, and checksum | Desktop terminal lifecycle and protected approval flow |

Start with [Quick start](../start/quickstart.en.md) for prerequisites and the exact API Demo build command. For every example, the package is local until you select it through the NoriShell import flow; no example build command publishes or installs it.

Build commands in the focused pages are runnable from the stated NoriShell checkout root. Rust, JSON, JavaScript, and event-flow blocks are explanatory excerpts unless a page explicitly says otherwise; retain the corresponding source example's imports, state, manifests, assets, and validation rather than pasting an isolated block as a complete plugin.

## The common message loop

Most user-facing plugins follow this sequence:

```text
Initialize
  -> ui.document (Core renders an allowed target)
UiAction from an explicit click
  -> api.request (typed operation; no ambient grant)
BrokerResult from Core
  -> ui.document (replace or update the visible state)
```

`BrokerResult` is the point where the guest converts a typed success or failure into its next document. Do not fabricate success before that callback, and do not ask for protected approval from an initialization, timer, provider callback, or other background path. Read [calling Core](../development/calling-api.en.md), [UI documents](../development/ui.en.md), [errors](../development/errors.en.md), and [packaging](../development/packaging.en.md) alongside the focused example.
