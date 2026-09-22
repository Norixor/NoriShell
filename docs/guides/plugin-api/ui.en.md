# Declarative UI, fields, and targets

The public document schema is versioned and host-rendered. Use `stack`, `grid`, `section`, `divider`, `dialog` where permitted; presentation nodes such as text/code/status/progress; and bounded controls such as button/copy button/text field/select/checkbox/switch/table/menu/disclosure. Core rejects unknown node kinds, cross-document fields, unapproved targets, and stale revisions.

Targets come only from `ExtensionTargetRegistry`. A target has a stable id, surface kind, allowed node set, typed contextual projection, size budget, and risk level. The target location grants no terminal, Host, filesystem, or network access. `ui.page` and controlled plugin navigation remain in the plugin area. A plugin cannot insert primary navigation, arbitrary header controls, host routes, or DOM.

Page password fields exist only to feed the protected credential flow for the same action: no prefill, echo, Wasm value, state persistence, or storage. All other inputs remain ordinary bounded values. Use the application locale supplied by Core; plugin text does not set application locale. A declared `onOpenActionId` is still a background action: it may refresh a document using non-interactive data, but it may not open a protected prompt, create/unlock Vault, obtain credentials, or request terminal input.

A Page `onOpenActionId` declares a separate fieldless lifecycle hook. Its ID must not overlap any clickable action, including disabled buttons, table rows, or nested tree nodes. Core checks initial and replacement documents and derives background authority from the admitted Page declaration; a renderer flag cannot upgrade it to an explicit user action. Closing the target context fences in-flight synchronization.
