# Core workflow example

This protocol 1.13 plugin starts a package-declared task with three steps: inspect the API, create a three-second timer, and inspect again after Core delivers the timer event. The persistent Wasm instance returns a waiting response between the timer start and event; it does not sleep or poll in Wasm.

Build from the repository root with the installed Rust toolchain:

```sh
RUSTC="$(rustup which --toolchain 1.97.1 rustc)" CARGO_INCREMENTAL=0 cargo build --manifest-path examples/plugins/workflow-demo/Cargo.toml --target wasm32-unknown-unknown --release
cargo run -p norishell-plugin-sdk --bin norishell-plugin-dev -- pack examples/plugins/workflow-demo --wasm examples/plugins/workflow-demo/target/wasm32-unknown-unknown/release/norishell_workflow_demo.wasm --output /tmp/NoriShell-Workflow-Demo-1.0.0.zip
cargo run -p norishell-plugin-sdk --bin norishell-plugin-dev -- check /tmp/NoriShell-Workflow-Demo-1.0.0.zip
```

Import the ZIP using the application's local package flow. Open **Workflow demo**, select **Start task**, then **Refresh tasks** to inspect the actual Core record. While the timer is waiting, select **Refresh tasks** and then **Cancel task** to exercise scoped resource cleanup. Cancellation uses the displayed task revision; if Core has advanced it, `Conflict` means the snapshot is stale. Refresh before trying again; a completed task can no longer be cancelled. The example also offers a revision-fenced resume button when a task reports `needsUserAction`; this task itself needs no network, file or process permission.

Only task and step metadata is durable. Request payloads, replies and Wasm continuations remain in memory. Restarting the app reconciles an unfinished task to `interrupted`; it never replays earlier steps. Refresh can read that record, and starting again creates a new operation.

The guest unit test and a successful Wasm build/package check verify the example's event logic and ABI. They do not establish native Core task execution or restart acceptance; those are separate application checks.
