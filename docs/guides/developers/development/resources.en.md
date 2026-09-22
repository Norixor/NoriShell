# Resources, permissions and state

Declare the capabilities your plugin needs, then obtain approval for individual operations at runtime. Keep resource handles, task state and durable data separate so disable, revocation or restart cannot accidentally repeat an old operation.

## Permission layers

| Layer | Decision owner | What the plugin author does |
| --- | --- | --- |
| Manifest capability | Declared by the plugin; approved by the user through the host | Request only the capabilities needed by the feature; declaration does not grant access |
| Operation approval | Core evaluates the exact target, arguments and context | Trigger the request from an explicit user action and let the host show the target and risk |
| Remembered operation | The user chooses an expiry in the host prompt | Read non-secret summaries with `permissions`; current capability and execution checks still apply |

“Always allow” is not a boolean stored by the plugin. It applies only to an exact operation recognized by the host and can expire or be revoked. Do not build your own checkbox to approve file, network or process access on the user's behalf.

| Action | API | Next step |
| --- | --- | --- |
| Discover methods and capabilities | [describe](../../plugin-api/describe.en.md) | Use `availability` to display features and `limits` to bound requests |
| Read permissions and operation summaries | [permissions](../../plugin-api/permissions.en.md) | Keep the current `policyRevision` for management operations |
| Request a capability | [permissionRequest](../../plugin-api/permissionRequest.en.md) | Host declarative entry only; a direct Wasm call returns `unsupported`. Obtain grants through the installation/management flow |
| Revoke one remembered operation | [permissionRevoke](../../plugin-api/permissionRevoke.en.md) | Send the revision you read; refresh on conflict |
| Forget operations owned by the current package | [permissionsForget](../../plugin-api/permissionsForget.en.md) | Refresh the list; this does not expand or replace capability grants |

## A resource's complete lifecycle

```text
User initiates an operation
  → Request a resource
  → Receive an opaque handle
  → Read events and update the interface
  → Complete, cancel or fail
  → Close the resource and clear the in-memory handle
```

| Stage | Methods | What to verify |
| --- | --- | --- |
| Create | `networkStart`, `processStart`, `remoteExecStart`, `serialOpen`, `timerStart`, `subscriptionStart`, etc. | The result variant and handle belong to the current call |
| Send or continue | The corresponding `*Send` or resource API | Local acceptance does not mean a remote peer received or executed the request |
| Receive | [resourceEvents](../../plugin-api/resourceEvents.en.md) | Event order, bounded data, errors, exit and close states; handle backpressure where reported |
| Inspect current resources | [resourcesList](../../plugin-api/resourcesList.en.md) | Use only resources still owned by the current instance |
| Finish | [resourceClose](../../plugin-api/resourceClose.en.md) | Close explicitly and stop further writes; show failures or uncertain results |

Handles are opaque strings. Do not derive them from a filesystem path, device name, host ID or another plugin's output. Files and SFTP also have their own scoped handles and close operations; follow the sub-operation reference for [file](../../plugin-api/file.en.md) and [sftp](../../plugin-api/sftp.en.md).

## What can be persisted

| Data | Storage location | After restart |
| --- | --- | --- |
| Current UI, pending calls and temporary results | Bounded Wasm instance state | Initialize again; do not assume the previous instance survives |
| Non-secret preferences and plugin data | [storage](../../plugin-api/storage.en.md) | Read the current revision and use CAS for mutations |
| Passwords, tokens and credentials | Host-managed [credential](../../plugin-api/credential.en.md) flows | Retain only permitted opaque references and state; never put secrets in storage |
| Network, file, serial and process handles | The current operation's in-memory state | Do not persist or reuse across instances |
| Workflow tasks | State returned by [taskGet](../../plugin-api/taskGet.en.md) and [taskList](../../plugin-api/taskList.en.md) | Incomplete tasks can be `interrupted` after restart; read state and let the user choose what to do |

On a storage revision conflict, read again rather than overwriting a newer update. Task input, preselected file mappings and runtime replies are not a durable payload for automatic replay. `taskResume` is a new explicit action, not a hidden resubmission of old requests after restart.

## When another user action is needed

| Result | Interface behavior |
| --- | --- |
| `interactionRequired` or task `needsUserAction` | Show an explicit Continue action that returns to the host approval flow |
| `vaultMissing`, `vaultLocked`, `vaultRequiresReload` | Describe the corresponding state and use host handling; never collect the Vault password |
| `revoked`, a closed resource or a replaced package | End the operation and discard stale handles; do not reopen resources in the background |
| `outcomeUnknown` | Explain that the result is unconfirmed and let the user check actual state before retrying |

Timers, automatic task steps, `onOpen` and recovery callbacks are not user approval. Return the current state and wait for an explicit click. This keeps refusal, revocation and restart understandable. See [Errors and troubleshooting](./errors.en.md) for the full error table.
