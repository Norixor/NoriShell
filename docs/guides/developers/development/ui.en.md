# Declarative UI documents

NoriShell plugins describe a small, host-rendered interface. A plugin returns a `ui.document` runtime output; it does not create DOM nodes, inject CSS, call Tauri IPC, or control the host window. Core validates the schema, target, granted capability, action, fields, and current target context before it renders or dispatches anything.

Start with an initial document in `initialize`. When the user acts, return either a replacement document or one typed broker request. Read the asynchronous result in `brokerResult`, update your instance state, and return a new **complete** document for the same target. For the surrounding callback protocol, read [Calling APIs and handling replies](./calling-api.en.md).

## Document shape

`targetId` chooses a host-owned mount; a `document` is a full snapshot, not a patch. A complete parseable example appears below.

| Field | Required | Meaning |
| --- | --- | --- |
| `targetId` | yes | One registered extension target from the table below. Plugins cannot invent targets. |
| `document.schemaVersion` | yes | `1` for the current declarative UI schema. |
| `document.rootNodeId` | yes | The `nodeId` of exactly one root node. |
| `document.nodes` | yes | Flat node array. Child-bearing nodes refer to other entries by `nodeId`. |

All node objects have `kind` and `nodeId`. IDs are plugin-local identifiers, not DOM selectors or host resource IDs. Use stable, bounded IDs such as `summary`, `refreshButton`, and `sync:refresh`; do not put whitespace, paths, markup, passwords, tokens, host IDs, or handles in them. Unknown fields are rejected.

`children` contains node IDs, never nested node JSON. It is valid only on the node kinds listed below; `tabs` keeps its children in each tab object. A document must have valid references and must not rely on a partial update preserving omitted nodes. Do not model arbitrary HTML layout, custom CSS, JavaScript, IPC, browser navigation, or host data with a document: those are not declarative UI features.

## The 24 node kinds

Use the exact lower-camel-case `kind` values. A `?` after a field means it is optional; every other listed field is required in addition to `kind` and `nodeId`.

| `kind` | Fields besides `kind`, `nodeId` | Purpose |
| --- | --- | --- |
| `stack` | `direction`, `align`, `gap`, `children` | Lay out child nodes horizontally or vertically. |
| `grid` | `columns`, `columnWeights?`, `gap`, `children` | Lay out child nodes in a host-managed grid. If supplied, `columnWeights` has one positive value per column. |
| `section` | `title?`, `children` | Group related content, optionally under a title. |
| `divider` | — | Separate adjacent host-rendered content. |
| `text` | `text`, `style`, `tone` | Display text using a host text style and tone. |
| `code` | `text`, `language?`, `wrap` | Display bounded code or diagnostic text; it is not an editable terminal. |
| `icon` | `icon`, `accessibleLabel`, `tone` | Display a host icon with an accessible name. |
| `status` | `label`, `tone` | Present a short status label. |
| `progress` | `label?`, `valuePercent?`, `tone` | Show host progress. Omit `valuePercent` when progress is not known. |
| `button` | `actionId`, `label`, `icon?`, `variant`, `disabled` | Send one explicit plugin UI action. |
| `copyButton` | `actionId`, `label`, `disabled` | Request a separate, host-checked clipboard write after an explicit action. |
| `textField` | `fieldId`, `label`, `value`, `placeholder?`, `fieldKind`, `required`, `disabled` | Collect text, search, number, URL, multiline, or page-only plugin-owned password input. |
| `select` | `fieldId`, `label`, `value?`, `options`, `disabled` | Select one declared option. Each option has `value`, `label`, and `disabled`. |
| `checkbox` | `fieldId`, `label`, `checked`, `disabled` | Collect a boolean choice. |
| `switch` | `fieldId`, `label`, `checked`, `disabled` | Collect a boolean setting with a switch presentation. |
| `table` | `label`, `columns`, `rows`, `emptyText?` | Render a bounded table. Columns have `columnId`, `label`, `width?`; rows have `rowId`, `cells`, `actionId?`. |
| `menu` | `label`, `children` | Group child actions in a host menu. |
| `dialog` | `title`, `description?`, `triggerLabel`, `closeLabel`, `children` | A host-owned, user-triggered dialog. The renderer owns open state, dismissal, and focus trapping. |
| `disclosure` | `label`, `open`, `children` | Show or hide a host disclosure section. |
| `sshSyncBrowser` | `profileId`, `children` | Page-only, host-owned SSH-sync data browser; the plugin receives neither its search, selection, nor pagination state. |
| `tabs` | `label`, `tabs` | Show host tabs. Each tab has `id`, `label`, and `children`. |
| `tree` | `label`, `items` | Render a tree. Each item has `id`, `label`, `children?`, and `actionId?`. |
| `chart` | `label`, `chartKind`, `series`, `labels` | Render host line or bar data. Each series has `label` and finite numeric `values`. |
| `editor` | `fieldId`, `label`, `value`, `language`, `readOnly` | Render a host editor field. |

The enum values are fixed: `direction` is `horizontal` or `vertical`; `align` is `start`, `center`, `end`, or `stretch`; text `style` is `body`, `secondary`, `caption`, `heading`, or `monospace`; `tone` is `neutral`, `info`, `success`, `warning`, or `danger`; button `variant` is `primary`, `secondary`, `ghost`, or `danger`; `fieldKind` is `text`, `search`, `number`, `url`, `multiline`, or `password`; and `chartKind` is `line` or `bar`.

### Fields, forms, and protected surfaces

Only `textField`, `select`, `checkbox`, `switch`, and `editor` produce field values. Core sends the current fields only with a validated action and filters them to the document that supplied that action. Treat each value as untrusted user input, validate it for the operation, and never use it to impersonate a user or choose an unrestricted host resource.

Form nodes require a target whose `forms` column is **yes**. `textField.fieldKind: "password"` is allowed only in a plugin-owned `app.page`, and it is for a plugin's own credential flow. It is not a way to read a Vault, SSH credential, another application, or a host password. `sshSyncBrowser` is also only allowed on `app.page`; its remote-data interaction remains Core-owned. A `dialog` may be contributed at `app.header.actions` or on `app.page`; it never gives the plugin window, focus, or dismissal control.

## Registered mount targets

The registry below is the complete target list. `Context` means the renderer opens a concrete, short-lived target instance before it asks the plugin for a contribution. `Forms` means regular form node kinds are admitted there. “No” is a boundary, not an invitation to simulate a form with text or custom markup.

| Target | Surface | Required capability | Context | Forms |
| --- | --- | --- | --- | --- |
| `plugins.page` | inline | `uiPanel` | no | yes |
| `app.header.actions` | toolbar | `uiPanel` | no | no |
| `app.content.before` | inline | `uiPanel` | yes | yes |
| `app.content.after` | inline | `uiPanel` | yes | yes |
| `app.content.sidebar` | sidebar | `uiPanel` | yes | yes |
| `app.content.footer` | inline | `uiPanel` | yes | yes |
| `app.content.floating` | overlay | `uiPanel` | yes | yes |
| `terminal.tools` | sidebar | `uiPanel` | yes | yes |
| `terminal.header` | inline | `uiPanel` | yes | yes |
| `terminal.footer` | inline | `uiPanel` | yes | yes |
| `terminal.floating` | overlay | `uiPanel` | yes | yes |
| `terminal.toolbar` | toolbar | `uiPanel` | yes | yes |
| `terminal.sidebar` | sidebar | `uiPanel` | yes | yes |
| `terminal.contextMenu` | menu | `uiPanel` | yes | no |
| `terminal.annotation` | overlay | `terminalAnnotation` | yes | no |
| `sftp.toolbar` | toolbar | `uiPanel` | yes | yes |
| `sftp.contextMenu` | menu | `uiPanel` | yes | no |
| `sftp.transfer.actions` | inline | `uiPanel` | yes | no |
| `hosts.toolbar` | toolbar | `uiPanel` | no | yes |
| `host.detail.tools` | inline | `uiPanel` | yes | yes |
| `overview.toolbar` | toolbar | `uiPanel` | no | yes |
| `overview.card.actions` | card | `uiPanel` | yes | no |
| `tunnels.toolbar` | toolbar | `uiPanel` | no | yes |
| `settings.tools` | inline | `uiPanel` | no | yes |
| `commandPalette` | menu | `uiPanel` | no | yes |
| `app.navigation` | navigation | `uiNavigation` | no | no |
| `app.page` | page | `uiPage` | yes | yes |

`routePaths?` belongs to a UI template for ordinary application content and is an exact list of allowed application paths; omit it only to apply the template to every permitted ordinary mount. `onOpenActionId?` is a no-field visibility hook, separate from a user action. `autoRefresh?` is host-scheduled only for `terminal.footer`; it carries `actionId` and `intervalMs`, and must not stand in for user approval.

## Target-context boundary

Core supplies a contextual target as `targetId`, `surfaceKind`, `contextHandle`, `targetRevision`, and `displayLabel?`. The public UI-action request calls the opaque handle `contextHandle`; some host integrations describe the same concept as `targetContextHandle`. In either spelling, it is not plugin data.

Do not create, parse, store, guess, share, or revive a context handle. It is short-lived and fenced by owner, plugin generation, target, and target revision. On every UI action Core checks the current handle and revision; a stale target is a conflict to refresh, not a reason to replay an action. A contextual plugin should keep any temporary state scoped to the supplied target and discard it when that target closes. For `app.page`, Core additionally binds the active context to the requesting plugin and page identity.

The plugin receives only the host metadata or DOM snapshot that Core has explicitly authorized for that action. A target context never grants broad DOM inspection, CSS injection, IPC, host navigation, terminal access, credential access, or a way to reach another target.

## A minimal complete dialog document

This is a parseable `ui.document` payload with a `dialog`, `button`, and `code` node. It uses `app.header.actions`, where the dialog is allowed, but contains no form fields. All child nodes are present in the same document.

```json
{
  "targetId": "app.header.actions",
  "document": {
    "schemaVersion": 1,
    "rootNodeId": "apiDemoDialog",
    "nodes": [
      {
        "kind": "dialog",
        "nodeId": "apiDemoDialog",
        "title": "NoriShell API demo",
        "description": "Ask Core for the typed plugin API description.",
        "triggerLabel": "API demo",
        "closeLabel": "Close API demo",
        "children": ["intro", "describe", "details"]
      },
      {
        "kind": "text",
        "nodeId": "intro",
        "text": "No host, terminal, file, network, or Vault access is implied.",
        "style": "caption",
        "tone": "neutral"
      },
      {
        "kind": "button",
        "nodeId": "describe",
        "actionId": "api-demo:describe",
        "label": "Query API",
        "icon": "info",
        "variant": "primary",
        "disabled": false
      },
      {
        "kind": "code",
        "nodeId": "details",
        "text": "Select Query API to ask Core for its typed API description.",
        "language": "text",
        "wrap": true
      }
    ]
  }
}
```

Use the SDK helper to produce it without hand-building escaped payload JSON:

```rust
use norishell_plugin_sdk::{output, PluginError, PluginRuntimeOutput};
use serde_json::json;

fn document_output(request_id: &str, details: &str)
    -> Result<PluginRuntimeOutput, PluginError>
{
    output(request_id, "ui.document", &json!({
        "targetId": "app.header.actions",
        "document": {
            "schemaVersion": 1,
            "rootNodeId": "apiDemoDialog",
            "nodes": [/* complete node array, including `details` */]
        }
    }))
}
```

The `/* ... */` is Rust source inside `json!`, not JSON. In the actual output, use the full node array above. See the complete working [API Demo](../examples/api-demo.en.md).

## Action, broker request, and full replacement

The common asynchronous sequence is:

```text
initialize -> ui.document(target A, initial complete document)
user click -> uiAction(actionId, current validated fields)
plugin -> api.request(callId, typed operation)
Core -> brokerResult(result.kind = "api", reply for callId)
plugin -> ui.document(target A, complete replacement document)
```

For a normal user-driven `UiAction`, return one complete document for the same target **or** start one explicit asynchronous broker operation. Do not return a document fragment, switch targets as a side effect, or show an operation as successful before `brokerResult`. When a broker request is pending, use the `requestId` from the current message for `api_request`; on callback, use that callback's new `requestId` for the replacement output.

```rust
use norishell_plugin_sdk::{
    api_request, payload, PluginApiOperation, PluginApiOutcome,
    PluginApiReply, PluginError, PluginHostMessageKind, PluginHostRequest,
    PluginRuntimeOutput,
};
use serde_json::Value;

const DESCRIBE_ACTION: &str = "api-demo:describe";

fn handle_ui(request: &PluginHostRequest) -> Result<Vec<PluginRuntimeOutput>, PluginError> {
    let body: Value = payload(request)?;
    match request.kind {
        PluginHostMessageKind::UiAction
            if body["actionId"].as_str() == Some(DESCRIBE_ACTION) =>
        {
            Ok(vec![api_request(
                &request.request_id,
                "api-demo.describe",
                PluginApiOperation::Describe {},
            )?])
        }
        PluginHostMessageKind::BrokerResult => {
            let result = &body["result"];
            if result["kind"].as_str() != Some("api") {
                return Err(PluginError::InvalidRequest);
            }
            let reply: PluginApiReply = serde_json::from_value(result["reply"].clone())
                .map_err(|_| PluginError::InvalidRequest)?;
            if reply.call_id != "api-demo.describe" {
                return Err(PluginError::InvalidRequest);
            }
            let details = match reply.outcome {
                PluginApiOutcome::Completed { value } => format!("{value:#?}"),
                PluginApiOutcome::Failed { code } => format!("Core rejected describe: {code:?}"),
            };
            // Update local non-secret state first, then replace target A in full.
            Ok(vec![document_output(&request.request_id, &details)?])
        }
        _ => Err(PluginError::InvalidRequest),
    }
}
```

`callId` correlates an operation; it is neither a capability grant nor a target handle. Use a distinct valid call ID for each outstanding operation and match the result before changing the affected UI state. Operations may fail, be denied, be revoked, conflict, or finish with an unknown outcome; surface the stable error and wait for an appropriate explicit retry. See [Resources and permissions](./resources.en.md) for operation and handle lifecycles, and [appRegister](../../plugin-api/appRegister.en.md) for an API method that requires a verified action.

## Practical checklist

1. Declare the target's required capability and wait for user approval.
2. Return one valid complete document during `initialize`.
3. Use only the 24 nodes and registered target IDs above; do not add HTML, CSS, DOM, IPC, or invented fields.
4. Keep field nodes on form-accepting targets; keep password and SSH-sync browser UI on `app.page`.
5. Treat `contextHandle` / `targetContextHandle`, revisions, fields, and host-provided metadata as scoped inputs, not reusable authority.
6. For asynchronous work, wait for the matching `brokerResult`, then replace the same target's document in full.
