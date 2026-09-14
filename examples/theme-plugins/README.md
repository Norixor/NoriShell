# NoriShell pure-data theme packages

These are three independent local ZIP examples for the theme-package branch:

- `clear`: cool white and blue light appearance.
- `midnight`: deep navy dark appearance.
- `sand`: warm ivory and brown-gold light appearance.

Each source package contains only `manifest.json` and `assets/theme.json`. It declares protocol `1.14`, no capabilities, and no Wasm, JavaScript, HTML, CSS, URLs, or arbitrary assets. `terminalPalette` follows the current terminal palette shape (21 existing color keys).

Run the deterministic builder from the repository root:

```sh
python3 examples/theme-plugins/build.py
pnpm vitest run --config examples/theme-plugins/vitest.config.ts
```

The builder writes versioned archives and adjacent lowercase SHA-256 files to `output/theme-plugins/`, pins ZIP entry timestamps and permissions, validates the full fixed schema and contrast rules, then checks that each archive has exactly the two permitted file entries. Re-running it with unchanged source produces the same archive bytes with the supported local Python runtime.

These checks validate source data and package structure. Core installation, private-copy/hash binding, lifecycle operations, and native-app acceptance remain Core and desktop verification work.
