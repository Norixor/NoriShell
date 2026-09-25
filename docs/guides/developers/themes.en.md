# Declarative theme plugins

A theme plugin is a bounded data package managed through the existing ZIP import, install, update, disable and uninstall flows. It does not run a Plugin Host or request capabilities. Installing a theme does not select it automatically: choose and save it in **Settings → Appearance**.

## Package contract

The ZIP contains only `manifest.json` and `assets/theme.json`, plus their directory entries. The existing manifest fields and signing format remain unchanged. Theme packages require protocol 1.14, empty capabilities, and no `plugin.wasm`. Existing Wasm packages retain the 1.13 ABI baseline. Mixed packages, additional files, old-protocol themes and nonempty capabilities are rejected.

Local ZIP publisher labels are self-reported. Theme content is bound to the package hash and its actual bytes are checked again on installed reads. A corrupt theme is unavailable and never falls back to Wasm execution.

Rebuildable examples under `examples/theme-plugins/` provide Moss (light), Mulberry (dark) and Sand (light). Follow that directory's script and README to produce versioned ZIPs. New hosts retain old Wasm compatibility; old hosts cannot use theme packages, and their exact error wording depends on the old version.

## Definition

`assets/theme.json` is at most 32 KiB, with these fields:

| Field | Contract |
| --- | --- |
| `schemaVersion` | `1` |
| `id` | Lowercase identifier, 1–64 characters |
| `name` | Bounded `zhCN` and `en` labels, rendered as text |
| `appearance` | `light` or `dark`, one theme per package |
| `colors` | All 26 semantic roles below; `#RRGGBB` only |
| `fontFamily` | `system`, `sans`, `mono`; host font stacks |
| `fontSize` | Integer 12–18 |
| `radius` | Integer 0–12 |
| `borderWidth` | 1 or 2 |
| `density` | `compact`, `standard`, `comfortable` |
| `shadow` | `none`, `soft`, `standard` |
| `terminalPalette` | Optional complete existing TerminalPalette |

Color roles: `bgCanvas`, `bgSurface`, `bgSubtle`, `bgHover`, `border`, `borderStrong`, `textPrimary`, `textSecondary`, `textTertiary`, `accent`, `accentHover`, `onAccent`, `accentSoft`, `success`, `successSoft`, `warning`, `warningSoft`, `danger`, `onDanger`, `dangerSoft`, `focusRing`, `terminalPaneActiveBorder`, `brandMarkPrimary`, `brandMarkSecondary`, `selection`, `selectionText`.

Unknown fields and arbitrary CSS are rejected. The host checks text, button, selection, focus and status contrast. CSS, scripts, HTML, font files, URLs, selectors, stacking order, hiding content and behavior changes are not theme capabilities.

## Preferences and isolation

Appearance mode is saved independently. Users assign light and dark themes separately for system mode, and customize semantic colors and bounded typography/geometry in a draft. Import also loads a draft. Only successful persistence applies changes. Overrides are bound to plugin and theme identity and never modify package files. Profile exports contain selections and overrides, not code or embedded external definitions; existing preference imports remain supported.

Disabled, removed or corrupt themes fall back to a built-in theme of the same appearance while preserving customizations. Updated definitions and overrides are revalidated. Terminal palettes remain independent when explicitly selected; follow-app may use the theme's optional palette. Recoloring does not recreate sessions or processes.

Shared ordinary UI inherits the theme. Isolated custom web pages must explicitly integrate appearance context. OS dialogs and native controls remain platform-controlled. Security windows and protected in-page regions retain built-in color and layout tokens. The ordinary tray consumes only a validated resolved appearance projection, without loading plugins.

Required checks include old Wasm compatibility, malformed ZIP rejection, update rollback, zero theme processes, disable/uninstall fallback, restart, draft cancellation and save failure, system mode, terminal independence and native security surfaces. See [implementation status](../../../README.md#installation-and-quick-start) for actual evidence and unverified platforms; examples do not imply publication or cross-platform acceptance.

Theme isolation uses a separate `data-theme-protected` boundary. Existing `data-plugin-protected` marks plugin DOM access restrictions; ordinary terminals and plugin panels can still follow the selected theme. Plugin safe mode marks installed themes unavailable for activation and uses built-in appearance.
