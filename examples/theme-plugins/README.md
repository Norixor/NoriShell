# NoriShell pure-data theme packages

These are three independent local ZIP examples for the theme-package branch:

- `clear`: Moss (青苔) 1.1.0, sage surfaces and forest-green controls.
- `midnight`: Mulberry (绛夜) 1.1.0, plum surfaces and apricot controls.
- `sand`: warm ivory and brown-gold light appearance.

Each source package contains only `manifest.json` and `assets/theme.json`. It declares protocol `1.14`, no capabilities, and no Wasm, JavaScript, HTML, CSS, URLs, or arbitrary assets. `terminalPalette` follows the current terminal palette shape (21 existing color keys).

Run the deterministic builder from the repository root:

```sh
python3 examples/theme-plugins/build.py
pnpm vitest run --config examples/theme-plugins/vitest.config.ts
```

The builder writes versioned archives and adjacent lowercase SHA-256 files to `output/theme-plugins/`, pins ZIP entry timestamps and permissions, validates the full fixed schema and contrast rules, then checks that each archive has exactly the two permitted file entries. Re-running it with unchanged source produces the same archive bytes with the supported local Python runtime.

These checks validate source data and package structure. Core installation, private-copy/hash binding, lifecycle operations, and native-app acceptance remain Core and desktop verification work.

Moss and Mulberry replace the former Clear and Midnight packages. The `clear` / `midnight` source folders, plugin IDs, and theme IDs remain stable so saved selections continue to resolve without duplicate entries. Local ZIPs have package-bound trust: changing bytes cannot authorize an in-place update. For these local examples, uninstall the previous theme with data retention, then import and enable the new ZIP. A verified signed update requires the same publisher identity. User overrides remain stored; reset this theme to see the complete new default palette. Sand stays at 1.0.0.
