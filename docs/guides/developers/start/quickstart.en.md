# Quick start: your first visible plugin

This path starts with the repository's **API Demo**, because it creates a visible host-rendered dialog. The SDK `scaffold` command is useful next, but its default `example.ready` output is an ABI skeleton, not a visible application contribution.

## Before you build

This beta handbook targets NoriShell `0.1.0-beta`. API Demo source currently leaves `examples/plugins/api-demo/manifest.json` at `"minimumAppVersion": "0.1.0"`; before running its build command, change your local copy to `"minimumAppVersion": "0.1.0-beta"`. Otherwise this beta host rejects the ZIP.

You need all of the following on the development machine:

| Requirement | Why it is needed |
| --- | --- |
| A local NoriShell source checkout that contains `crates/plugin-sdk` and `examples/plugins` | The SDK is currently delivered with the source tree and is a source dependency in that checkout. Obtain a source version that includes both directories before starting. |
| Python 3 | The API Demo build script is Python. |
| `rustup` toolchain `1.97.1` plus `wasm32-unknown-unknown` | The example compiles a Rust `cdylib` to Wasm with this target. |
| Cached Cargo dependencies | The API Demo script deliberately builds with `--locked --offline`; it will stop when the local cache is incomplete. |
| A matching NoriShell desktop application | You need it for the final import, approval, enablement, and visible-result check. |

Run the following commands **from the root of your local NoriShell checkout**. The repository's `rust-toolchain.toml` selects `1.97.1`; API Demo's build script explicitly invokes that toolchain, targets `wasm32-unknown-unknown`, and uses `--locked --offline` for both the demo build and its SDK verifier.

```sh
# Inspect the installed toolchain and Wasm target.
rustup toolchain list
rustup target list --toolchain 1.97.1 --installed

# Install the exact prerequisites if either check is missing them.
rustup toolchain install 1.97.1
rustup target add --toolchain 1.97.1 wasm32-unknown-unknown

# Populate the locked workspace dependency cache once while network access is available.
rustup run 1.97.1 cargo fetch --locked
```

`cargo fetch --locked` runs against the root workspace, which includes both `crates/plugin-sdk` and its `plugin-platform` dependency used by the verifier. Run it successfully before the offline build; do not add `--offline` to the fetch command. The source checkout must remain available while you build a plugin scaffold because `scaffold` writes an absolute path to its local `norishell-plugin-sdk` dependency.

Then verify the CLI and build the first package:

```sh
# Verify that the SDK development CLI in this checkout is available.
CARGO_INCREMENTAL=0 cargo run --locked -p norishell-plugin-sdk --bin norishell-plugin-dev -- --help

# Build a real, visible example package. The script also writes a .sha256 sidecar.
python3 examples/plugins/api-demo/build.py --output /tmp/NoriShell-API-Demo-1.0.1.zip

# Inspect the ZIP with the SDK package validator and production Wasm ABI runtime.
CARGO_INCREMENTAL=0 cargo run --locked -p norishell-plugin-sdk --bin norishell-plugin-dev -- \
  check /tmp/NoriShell-API-Demo-1.0.1.zip
```

The build script compiles the guest Wasm and runs the API Demo ABI verifier. `check` additionally opens the constrained ZIP, validates its manifest and assets, and loads `plugin.wasm` with the production runtime. Neither command installs the package, grants a capability, or talks to a live Core broker.

## See the result in NoriShell

1. Open **Plugins → Import ZIP** in a matching NoriShell desktop application and select `/tmp/NoriShell-API-Demo-1.0.1.zip`.
2. Read the package identity and requested capabilities. API Demo requests `uiPanel` and `clipboardWrite`.
3. Finish the local import, approve the requested capabilities in the host flow, and enable the plugin.
4. Open **API demo**, then select **Query API**. The dialog should replace its initial hint with the current Core plugin methods and limits.
5. Select **Copy API description** and compare the clipboard text with the displayed result. Disable the plugin and verify that its contribution disappears.

Opening the dialog alone does not make an API request. If the ZIP imports but nothing is visible, inspect the plugin's `ui.document` contribution and its target before treating “enabled” as a successful user flow. See [UI documents](../development/ui.en.md) and [the API Demo round trip](../examples/api-demo.en.md).

## Start your own project after the demo

The local CLI creates an empty, non-overwriting Rust/Wasm project. Run it from the NoriShell source checkout:

```sh
CARGO_INCREMENTAL=0 cargo run --locked -p norishell-plugin-sdk --bin norishell-plugin-dev -- \
  scaffold /tmp/norishell-example --id com.example.norishell-example --name "My NoriShell plugin"
```

It writes `Cargo.toml`, `manifest.json`, `src/lib.rs`, and a README. The generated Cargo manifest points to the current checkout with an absolute SDK path. Keep that checkout, or deliberately replace the dependency path when moving the project. The generated guest only returns `example.ready`, so follow API Demo's `Initialize` → `ui.document` pattern before expecting a UI.

Build the generated project, package it, and check the resulting new ZIP. `pack` refuses to overwrite an existing output file.

```sh
RUSTC="$(rustup which --toolchain 1.97.1 rustc)" CARGO_INCREMENTAL=0 cargo build --release \
  --target wasm32-unknown-unknown --target-dir /tmp/norishell-example-target \
  --manifest-path /tmp/norishell-example/Cargo.toml

CARGO_INCREMENTAL=0 cargo run --locked -p norishell-plugin-sdk --bin norishell-plugin-dev -- \
  pack /tmp/norishell-example \
  --wasm /tmp/norishell-example-target/wasm32-unknown-unknown/release/norishell_com_example_norishell_example.wasm \
  --output /tmp/norishell-example-0.1.0.zip

CARGO_INCREMENTAL=0 cargo run --locked -p norishell-plugin-sdk --bin norishell-plugin-dev -- \
  check /tmp/norishell-example-0.1.0.zip
```

Do not edit API Demo in place and install it under its original identity for unrelated experiments. Give your package its own `pluginId`, name, version, declared capabilities, actions, and assets. Continue with [language boundaries](./languages.en.md), [calling Core](../development/calling-api.en.md), and [packaging](../development/packaging.en.md).
