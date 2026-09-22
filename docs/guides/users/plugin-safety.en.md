# Plugin permissions and recovery

A plugin starts with no capability grant. It can propose a typed operation, but Core binds package identity, grant epoch, resource owner, and runtime generation before the operation is admitted. Closing a protected prompt means denial. A plugin cannot use a user-visible action as a substitute for a protected decision.

No permission exposes Vault passwords, KEK/VMK, Vault payload, raw SecretRef or CredentialRef values, SSH credentials, private keys, authentication answers, host-key decisions, SSH transports/channels, SQLite, arbitrary Tauri IPC, or the main Vue realm. Plugin-owned account credentials use a protected input flow and return only an opaque reference; the plugin never reads the secret. For HTTPS/WSS, Core injects that reference only into the approved exact origin.

Safe mode can be scheduled before plugin hosts, contributions, or owner CSS restore. It leaves management available so a package can be disabled or removed. A capability or API name does not prove a completed native acceptance; consult [implementation status](../../../README.en.md#installation-and-quick-start).

Approval is contextual. A protected window may offer `once` for the pending exact action or `always` only when Core can retain an exact-operation rule. An Always approval can last 15 minutes, 1 hour, 24 hours, or indefinitely; Core records a finite choice as an absolute deadline, so reopening the app does not renew it. “Always” does not expose a broad account, host, transport, or Vault permission. It remains revocable and is checked again when the action runs. Background refresh and `onOpen` never prompt for Vault creation or unlock; they show a state that a user can continue explicitly.

Remembered-operation management shows a non-secret description and its expiry state. A selected local file/directory or serial device is shown in saved history only as a generic label with an opaque suffix, never its raw path or device identity. An installed plugin can forget only decisions owned by its current signed package; the application may retain prior package history for review.
