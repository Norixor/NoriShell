# Service Demo: approved HTTPS in a workflow

Service Demo fetches public metadata for `rust-lang/rust` from GitHub through Core. The Wasm guest has no HTTP client, socket, proxy, credential, filesystem, or Tauri access. It starts a Core-owned workflow only after the user selects **Query GitHub**.

## What the user does

1. Build, check, import, approve `uiPanel` and `networkDomain`, then enable the package.
2. Open **Service demo** and select **Query GitHub**. This creates a task; it does not silently grant network access.
3. Select **Refresh** until the task requires action, then choose **Continue query**. Core, not the guest, presents the exact network approval.
4. Refresh after completion to show the HTTP status and validated repository name, description, and star count. Select **Cancel query** while active to exercise revision-fenced cancellation.

The guest accepts only a 200 JSON response, caps its accumulated body at 64 KiB, validates UTF-8 and expected fields, and leaves response data in the live Wasm instance. It does not put it in plugin storage, task persistence, or logs. The Rust line below is a concept-only excerpt, not compilable source: a real `NetworkStart` needs the SDK endpoint and HTTP request DTO values shown in the source example.

```rust
// The important request is typed; Core resolves and reviews the endpoint.
PluginApiOperation::NetworkStart { /* endpoint and HTTP request DTOs */ }
```

The likely results are deliberately distinct: `needsUserAction` means render an explicit **Continue query** control; a completed task means refresh and show the validated response; a failed or cancelled task means render that state. A returned resource handle is Core's local acceptance, not proof that the remote server completed a request.

| If you see… | Input and first check |
| --- | --- |
| **Continue query** remains disabled | Start the task first, then refresh until Core returns the task state that needs an explicit user action. |
| A prompt appears | The required input is the user's approval of the exact GitHub HTTPS operation; do not collect a credential in the guest. |
| The task completes but no repository data appears | Refresh the task result; only a complete 200 JSON response with the expected fields renders the report. |
| Cancellation returns `conflict` | Refresh the current task snapshot, then send a new cancel request with its displayed revision. |

## Build boundary

From the NoriShell checkout root:

```sh
python3 examples/plugins/service-demo/build.py --output /tmp/NoriShell-Service-Demo-1.0.1.zip
```

The script runs guest tests, builds Wasm with the repository toolchain, uses the SDK deterministic `pack`, runs SDK `check`, and writes a checksum sidecar. It requires the exact output filename shown above and refuses to overwrite it. That evidence does not replace live desktop approval, GitHub HTTPS, cancellation, or resource-cleanup acceptance.

Use [calling Core](../development/calling-api.en.md) for typed network operations, [errors](../development/errors.en.md) for `interactionRequired` and task states, and [packaging](../development/packaging.en.md) before adapting the sample.
