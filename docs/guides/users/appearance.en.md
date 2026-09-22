# Themes and appearance

1. Import and enable a theme ZIP on the Plugins page. Installation does not select it automatically. Moss and Mulberry 1.1.0 and Sand 1.0.0 are available as local examples; see the [theme developer guide](../developers/themes.en.md) for build instructions.
2. Open Settings → Appearance and browse all themes together, labeled Light or Dark. Selecting a card previews it; saving switches to its appearance mode. The mode dropdown saves immediately; System uses the most recently saved light and dark selections.
3. Adjust semantic colors, fonts, body size, corners, borders, density and shadows. Check the component and list previews. Edits stay in a draft until Save changes; Discard changes restores the saved profile.
4. Reset this theme removes only its draft overrides; save to apply. Insufficient color contrast blocks saving, so check foreground and background colors together.

Import profile loads a draft. Export saved profile exports the applied profile, excluding unsaved edits and the theme packages themselves. Install the corresponding ZIP separately on another device. Missing, disabled or corrupt themes and plugin safe mode use built-in appearance while retaining your overrides. Overrides incompatible with an updated theme are retained but temporarily not applied.

Terminal appearance is managed independently in Terminal preferences. Follow application can use a package-provided palette; fixed and custom terminal palettes remain independent. A color change never creates, reconnects or closes a terminal session.

Security, credential and permission confirmations retain built-in appearance. Native controls and file dialogs follow platform limits. Ordinary declarative plugin panels can follow the theme; independently drawn isolated pages require plugin support. See [implementation status](../../../README.en.md#installation-and-quick-start) for current platform acceptance.

Moss and Mulberry retain the former Clear/Midnight theme IDs. A local ZIP cannot authorize a changed package through a self-reported publisher: uninstall the old theme while retaining data, then import and enable the new package. Existing overrides remain; reset the theme and save to use its complete new defaults.
