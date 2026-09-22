# Workflow Demo: wait for Core instead of sleeping in Wasm

Workflow Demo declares a three-step task: inspect the API, start a three-second timer, then inspect the API again after Core delivers the timer event. The guest returns a waiting workflow response between the timer request and the event; it never sleeps or polls inside Wasm. The sequence below explains the full event loop; it is not standalone Rust source.

## Expected user flow

After import and enablement, open **Workflow demo** and select **Start task**. Select **Refresh tasks** to inspect the current Core task record. While the timer is waiting, refresh and then select **Cancel task** to check scoped resource cleanup. Cancellation includes the displayed task revision; `conflict` means the snapshot is stale, so refresh before taking a new action.

When Core reports `needsUserAction`, the demo exposes a revision-fenced resume action. This particular task does not require network, file, or process permission.

```text
UiAction(Start task) -> api.request(taskStart)
BrokerResult(task snapshot) -> ui.document
WorkflowEvent(timer) -> workflow response / next typed call
BrokerResult -> ui.document with the new snapshot
```

Only task and step metadata are durable. Payloads, replies, and Wasm continuations remain in memory. After application restart, Core reconciles an unfinished task to `interrupted`; it does not replay an earlier task dispatch. Refresh can show that record, and starting again creates a new operation.

| If you see… | Input and expected result |
| --- | --- |
| No task after **Start task** | The action needs a visible, enabled package and a valid `taskStart` request; then refresh to read Core's task record. |
| Timer appears to do nothing | Wait for Core's timer event and refresh; the guest does not own a background sleep loop. |
| `conflict` on cancel or resume | Refresh first. The next user click must use the latest task revision. |
| App restarted mid-task | Expect `interrupted`, not automatic replay; start a new task if the user wants another run. |

## Build and package

Run these commands from the NoriShell checkout root:

```sh
RUSTC="$(rustup which --toolchain 1.97.1 rustc)" CARGO_INCREMENTAL=0 cargo build \
  --manifest-path examples/plugins/workflow-demo/Cargo.toml \
  --target wasm32-unknown-unknown --release
cargo run -p norishell-plugin-sdk --bin norishell-plugin-dev -- \
  pack examples/plugins/workflow-demo \
  --wasm examples/plugins/workflow-demo/target/wasm32-unknown-unknown/release/norishell_workflow_demo.wasm \
  --output /tmp/NoriShell-Workflow-Demo-1.0.0.zip
cargo run -p norishell-plugin-sdk --bin norishell-plugin-dev -- \
  check /tmp/NoriShell-Workflow-Demo-1.0.0.zip
```

The Wasm build and package check validate local artifact boundaries. Import, task execution, cancellation, and restart behavior still need verification in NoriShell. Read [calling Core](../development/calling-api.en.md) and [errors](../development/errors.en.md) when adapting task and revision handling.
