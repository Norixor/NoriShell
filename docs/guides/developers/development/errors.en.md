# Errors and troubleshooting

API failures return a stable code through `PluginApiOutcome::Failed { code }`. Treat failure as part of the user flow: retain the necessary state, explain what happens next, and do not replace an error with a blank view or false success.

## Error codes

| Code | Meaning | Recommended handling |
| --- | --- | --- |
| `invalidRequest` | Invalid fields, format or context | Check the method table, enum `kind`, field names, callId and size limits |
| `permissionDenied` | Missing permission or an unapproved operation | Explain the failure and check the manifest/capability; let the user decide whether to authorize again |
| `interactionRequired` | A host operation needs user participation | Show an explicit Continue button; do not request dialogs in a background loop |
| `vaultMissing` | No Vault has been established | Describe the state and use the host setup flow; the plugin does not create the Vault |
| `vaultLocked` | The Vault is locked | Let the host unlock it after an explicit click; never collect the password in the plugin |
| `vaultRequiresReload` | Vault state needs reloading | Let the host recover, then start a new explicit action; do not treat this as a missing Vault |
| `unsupported` | Unsupported request or capability | Query `describe` and disable the unsupported feature; do not guess a compatibility API |
| `revoked` | Permission or resource eligibility was revoked | Stop, discard stale handles, and do not reuse a remembered string as permission |
| `notFound` | A record or resource is missing | Refresh the target list and stop using the stale reference |
| `conflict` | Revision or state conflict | Read current state and let the user confirm changes that need resubmission |
| `busy` | A resource or operation is busy | Show a waiting state or explicit retry; avoid unbounded concurrent retries |
| `quotaExceeded` | Runtime quota exceeded | Reduce size, process bounded chunks or release unused resources; consult `describe.limits` |
| `timedOut` | The operation timed out | Show the timeout and check possible side effects before retrying writes |
| `cancelled` | The operation was cancelled | Stop waiting and clean up current state and resources |
| `outcomeUnknown` | It is unclear whether side effects occurred | Check the actual result before deciding to retry |
| `cleanupIncomplete` | Cleanup did not fully complete | Show the incomplete state and inspect current resources; do not claim everything was released |
| `unavailable` | A service or resource is unavailable | Keep recoverable input, explain the state and offer an explicit retry |

This is the complete error enum, not a promise that every method returns every code. Individual method pages explain the relevant permissions, results and resource handling.

## Diagnose by symptom

| Symptom | Check first | Reference |
| --- | --- | --- |
| Build script cannot find dependencies or the Wasm target | Rust toolchain, target and Cargo cache; this is not a runtime permission failure | [Quick start](../start/quickstart.en.md) |
| ZIP is rejected | Required manifest fields, protocol 1.13, package paths, size and platform | [Packaging and installation](./packaging.en.md) |
| Enabled plugin has no visible UI | A valid `ui.document` from `initialize`, a registered target and approved capability | [Declarative UI](./ui.en.md) |
| UI disappears or stays unchanged after a click | A normal `uiAction` must return a full document for the same target; asynchronous replies update in `brokerResult` | [Calls and replies](./calling-api.en.md) |
| A sent request has no matching result | callId versus requestId, `result.kind` and the reply's tagged union | [Calls and replies](./calling-api.en.md) |
| Network or process handle exists but no result appears | Resource events, HTTP status, exit code, closure and backpressure | [Resources and permissions](./resources.en.md) |
| Settings updates keep conflicting | Reuse of an old revision | [storage](../../plugin-api/storage.en.md) |
| Task is interrupted after restart | Durable task state versus in-process payload; accidental replay of old requests | [Workflow example](../examples/workflow.en.md) |

## Validate your plugin

| Stage | Evidence to look for | What it does not prove |
| --- | --- | --- |
| Rust compilation | Correct types and call arguments | No actual Core operation has run |
| SDK `check` / ABI harness | The current runtime accepts the package and Wasm ABI | This is not host permission approval or a visible desktop result |
| Install, enable and use in the application | Correct UI, actual results and failure feedback | Success in one environment does not establish every platform or device |
| Refuse, revoke, cancel and restart | Operations stop, state stays understandable and resources are handled | “Did not crash” is not a result check |

Log method names, callIds and stable error codes when troubleshooting. Avoid passwords, authentication headers, user input and entire upstream responses. Check actual side effects before retrying file writes, process execution or network operations.
