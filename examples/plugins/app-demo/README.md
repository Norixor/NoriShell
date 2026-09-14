# App integration example

This protocol 1.13 plugin exercises explicit app registration, a command and
optional shortcut, a status item, an in-app notification, controlled navigation,
and a file command. Initialization only renders the dialog. **Register app
command** asks Core to register **Open text file**; the host command invokes the
same declared action as **Select text file**. The action uses the native
`FilePick` broker with read-only access and a protected exact-file approval.
Extension labels do not create OS file associations or grant file access.

After selection, **Read preview** reads the first bounded chunk through the
opaque file handle and displays at most 2,048 characters. **Close file handle**
closes the Core resource. A second selection requires closing the first handle.
There is no automatic file access, network access, or Vault access. Register,
notify, navigation and file selection each require an explicit click. In the
host command palette the user can enable the optional Alt+Shift+O binding.

Build and package from the repository root:

```sh
RUSTC="$(rustup which --toolchain 1.97.1 rustc)" CARGO_INCREMENTAL=0 cargo build --manifest-path examples/plugins/app-demo/Cargo.toml --target wasm32-unknown-unknown --release
cargo run -p norishell-plugin-sdk --bin norishell-plugin-dev -- pack examples/plugins/app-demo --wasm examples/plugins/app-demo/target/wasm32-unknown-unknown/release/norishell_app_demo.wasm --output /tmp/NoriShell-App-Demo-1.0.0.zip
cargo run -p norishell-plugin-sdk --bin norishell-plugin-dev -- check /tmp/NoriShell-App-Demo-1.0.0.zip
```

The build and package checks do not replace native selection, protected approval,
command/shortcut, notification and cleanup acceptance.
