# Protocol providers, serial resources, tasks, and app integrations

`protocolOpen` is the typed entry to a plugin protocol provider. Core owns the network/device broker and lifecycle; the resident Wasm instance receives `PluginProtocolEvent` and responds with typed `PluginProtocolOutput` or one broker continuation. It never claims a Core session identity or creates a raw socket. `serialDevices`, `serialOpen`, and `serialSend` use a Core-owned serial resource with selected-candidate identity, validated settings, bounded sends, close/revoke fencing, and ordered resource events.

`taskStart`, `taskGet`, `taskList`, `taskCancel`, and `taskResume` operate only on the package workflow catalog. A task gets one `PluginWorkflowEvent { taskId, workflowId, event }` at a time and may emit at most one typed broker call for its fixed step. A `WorkflowResponse` may deliberately wait (`complete: false`, no call, no step) only for a task-owned resource event; Core rejects a wait with no owned resource. Its persisted summary/step timestamps are JavaScript numbers. It has no persisted payload or continuation interpreter: startup changes unfinished durable rows to `interrupted`; it never replays a dispatch or restores its old input.

`appRegister`, `appNotify`, and `appNavigate` are explicit user-visible integration actions and receive `appAccepted` only after Core validates the package owner and current invocation. They are not registered by `Initialize`, page load, `onOpen`, or any implicit background execution.

`appNavigate` accepts only controlled application routes or the plugin’s own page. The main window may close declarative dialogs owned by the same plugin, package hash, and instance generation before navigating. Unknown dialogs, another plugin’s dialogs, and application security dialogs discard navigation immediately; dismissal does not replay it. Core ownership and dialog blocking are checked again after closing the owned dialogs.

Core integration fixtures have exercised the actual ProtocolSessionActor and task execution/cancellation; macOS native workflow checks cover execution, cancellation after refresh, cancellation on normal exit, and interrupted recovery after abnormal exit. The service example has completed a real HTTPS query after protected approval. Remaining platform, protocol, and hardware checks are tracked in [implementation status](../../../README.md#installation-and-quick-start). Package authors must still query `describe`; source availability, package validation, and unit tests do not replace the relevant native acceptance.

## Declare the provider catalog

Declare protocol providers at the fixed ZIP path `assets/protocols.json`. Installation validates this catalog; it is not a runtime registration interface. This is the catalog used by Protocol Demo:

```json
{
  "schemaVersion": 1,
  "providers": [
    {
      "id": "framedTcp",
      "label": {
        "en": "Framed TCP demo",
        "zh-CN": "Framed TCP 示例"
      },
      "configuration": {
        "schemaVersion": 1,
        "fields": [
          {
            "type": "string",
            "key": "endpoint",
            "label": {
              "en": "TCP endpoint",
              "zh-CN": "TCP 端点"
            },
            "default": "tcp://127.0.0.1:19071",
            "maxLength": 255
          }
        ]
      },
      "features": {
        "terminal": true,
        "resize": "supported",
        "reconnect": true
      },
      "resources": ["tcp"]
    }
  ]
}
```

Installation requires catalog `schemaVersion: 1`, at most 64 KiB for both raw and normalized JSON, and at most eight providers. Each unique `id` is 1–64 ASCII bytes, starts with a letter or digit, and contains only letters, digits, `.`, `_`, or `-`. Both `label.en` and `label.zh-CN` are required, non-empty, and limited to 512 bytes each; control and bidirectional formatting characters are rejected.

`configuration` reuses plugin settings schema v1 validation: 1–32 fields, unique non-secret keys, and valid defaults and field constraints. `features.terminal` must be `true`; `resize` is `supported` or `unsupported`; `reconnect` is boolean. `resources` accepts only `tcp`, `tls`, `websocket`, and `serial`, with at most four distinct entries. Unknown fields are rejected. Resource declarations do not grant access: the manifest must request the required capabilities, and runtime access still passes through Core broker authorization.

See [Protocol Demo](../../../examples/plugins/protocol-demo/README.md) for building, input/resize and closing. Repackage after catalog changes; replacing Wasm alone cannot update an installed catalog.
