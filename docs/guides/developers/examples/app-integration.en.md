# App integration: commands, a picker, and opaque handles

App Demo shows the Core-owned application integrations a plugin can request after explicit user actions: app registration, a command with an optional shortcut, a status item, notification, controlled navigation, and a read-only file workflow.

Initialization only renders the dialog. **Register app command** asks Core to register **Open text file**; the host command maps to the same declared action as **Select text file**. It does not create an operating-system file association, and the optional Alt+Shift+O shortcut must be enabled by the user in the host command palette.

The selected-file flow is deliberately staged. This is an event-flow excerpt, not standalone Rust source:

```text
Select text file -> api.request(filePick) -> BrokerResult(rootHandle)
Read preview -> api.request(file read) -> BrokerResult(data)
Close file handle -> api.request(resourceClose) -> BrokerResult(closed)
```

The native picker and exact file-scope approval belong to Core. The guest receives only an opaque root handle, reads one bounded chunk on a second click, displays at most 2,048 characters, and closes the resource explicitly. A second selection stays unavailable until the first handle is closed.

| If you see… | Input and expected result |
| --- | --- |
| **Open text file** is absent | Select **Register app command** first; host registration is an explicit action, not initialization behavior. |
| The picker asks for approval | The required input is the user's exact read-only file selection approval. The guest must receive only the returned opaque handle. |
| **Read preview** is unavailable | Select a text file first; after the read result, expect at most 2,048 characters in the dialog. |
| A second selection is blocked | Close the first file handle explicitly, then select another file. |

Build from the NoriShell checkout root:

```sh
RUSTC="$(rustup which --toolchain 1.97.1 rustc)" CARGO_INCREMENTAL=0 cargo build \
  --manifest-path examples/plugins/app-demo/Cargo.toml --target wasm32-unknown-unknown --release
cargo run -p norishell-plugin-sdk --bin norishell-plugin-dev -- \
  pack examples/plugins/app-demo \
  --wasm examples/plugins/app-demo/target/wasm32-unknown-unknown/release/norishell_app_demo.wasm \
  --output /tmp/NoriShell-App-Demo-1.0.0.zip
cargo run -p norishell-plugin-sdk --bin norishell-plugin-dev -- check /tmp/NoriShell-App-Demo-1.0.0.zip
```

Local checks do not prove the native picker, approval, command/shortcut, notification, navigation, or cleanup. See [calling Core](../development/calling-api.en.md) and [errors](../development/errors.en.md) before copying this lifecycle.
