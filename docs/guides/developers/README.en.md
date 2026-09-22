# NoriShell plugin development guide

Start with a visible plugin, learn to call Core capabilities, build UI, manage permissions and resources, and package your plugin as a local ZIP.

## Getting started

- [Plugin model and reading paths](start/overview.en.md)
- [Quick start: your first visible plugin](start/quickstart.en.md)
- [Languages, Wasm, and isolated UI](start/languages.en.md)

## Building plugins

- [Calling APIs and handling results](development/calling-api.en.md)
- [Declarative UI and action callbacks](development/ui.en.md)
- [Permissions and resource lifecycles](development/resources.en.md)
- [Packaging, installation, and upgrades](development/packaging.en.md)
- [Errors and user continuation](development/errors.en.md)

## API reference

- [35 methods, grouped by capability](../plugin-api/README.en.md)
- [Requests, results, and nested types](../plugin-api/types.en.md)

## Examples

- [Choosing an example and finding its source](examples/README.en.md)
- [API Demo: query, display, and copy](examples/api-demo.en.md)
- [Service Demo: network integrations](examples/service-demo.en.md)
- [Workflow Demo: multi-step tasks](examples/workflow.en.md)
- [App integration: commands, notifications, navigation, and files](examples/app-integration.en.md)
- [Isolated UI: HTML, CSS, and JavaScript](examples/isolated-ui.en.md)
- [Protocol Demo: terminal protocol providers](examples/protocol.en.md)

## Additional references

- [Protocol 1.13 Wasm ABI](wasm-abi.en.md)
- [Declarative theme plugins](themes.en.md)
- [Security and release checklist](security.en.md)
- [SDK tools and development loop](../../../examples/plugins/sdk-tooling/README.md)

Plugins can only use the public Plugin API. The [Core internal API catalog](../core-api/README.en.md) is for application maintainers and is not callable by plugins.
