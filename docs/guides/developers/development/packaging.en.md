# Packaging, installation and updates

Your development directory can contain source in any language that produces the protocol-1.13 Wasm ABI, plus build scripts and tests. The ZIP imported into NoriShell contains only the runtime manifest, Wasm and static assets. The current SDK `pack` and `check` commands are Rust tooling; another implementation must produce the same validated package boundary.

## ZIP layout

```text
my-plugin-0.1.0.zip
├── manifest.json
├── plugin.wasm
└── assets/
    └── isolated/
        └── toolbox.html
```

Exactly one `manifest.json` and one `plugin.wasm` must be at the ZIP root. `assets/` is optional. An isolated page opened with `surfaceId: "toolbox"` needs `assets/isolated/toolbox.html`. Do not include Cargo source directories, native libraries, `node_modules` or installation scripts.

## Manifest fields

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `pluginId` | string | Yes | A unique, stable identity such as `com.example.inspector`; do not reuse an official example's ID |
| `name` | string | Yes | The user-facing plugin name |
| `publisher` | string | Yes | Publisher description; this text is not verified signing identity in a local package |
| `version` | string | Yes | Canonical semver, such as `0.1.0`; increment for updates |
| `protocolMajor` | integer | Yes | Currently `1` |
| `protocolMinor` | integer | Yes | The current resident-Wasm ABI is `13`. Major-1 hosts support the stable range from `13` through their current resident ABI minor; this host currently accepts `13`. |
| `platform` | string | Yes | `desktop` for current local packages |
| `architectures` | string[] | Yes | Supported architecture declaration; portable Wasm examples use `["universal"]` |
| `capabilities` | string[] | Yes | Requested capabilities, at most 32 without duplicates; declaration is not approval |
| `minimumAppVersion` | string | Yes | The minimum app version appropriate to the features used |
| `publisherKeyId` | string or null | No | Publisher signing metadata; do not invent a key ID |
| `publisherSignature` | string or null | No | The corresponding signature; a string alone does not establish verified local publisher identity |
| `packageUrl` | string or null | No | Package source metadata; local development packages may omit it, and it does not publish a package |

This local manifest is a starting point for a UI and private-storage plugin. Change the identity and version, then adjust capabilities to match the actual feature.

```json
{
  "pluginId": "com.example.inspector",
  "name": "My Inspector",
  "publisher": "Example Developer",
  "version": "0.1.0",
  "protocolMajor": 1,
  "protocolMinor": 13,
  "platform": "desktop",
  "architectures": ["universal"],
  "capabilities": ["uiPanel", "storagePlugin"],
  "minimumAppVersion": "0.1.0-beta"
}
```

## Build, pack and check

Create `/tmp/norishell-example` using the [quick start](../start/quickstart.en.md). Run these commands from the NoriShell source root; the Wasm filename corresponds to that example's identity.

```sh
RUSTC="$(rustup which --toolchain 1.97.1 rustc)" CARGO_INCREMENTAL=0 cargo build --release \
  --target wasm32-unknown-unknown \
  --target-dir /tmp/norishell-example-target \
  --manifest-path /tmp/norishell-example/Cargo.toml

CARGO_INCREMENTAL=0 cargo run --locked -p norishell-plugin-sdk --bin norishell-plugin-dev -- \
  pack /tmp/norishell-example \
  --wasm /tmp/norishell-example-target/wasm32-unknown-unknown/release/norishell_com_example_norishell_example.wasm \
  --output /tmp/norishell-example-0.1.0.zip

CARGO_INCREMENTAL=0 cargo run --locked -p norishell-plugin-sdk --bin norishell-plugin-dev -- \
  check /tmp/norishell-example-0.1.0.zip
```

| Command | Input | What success means |
| --- | --- | --- |
| `scaffold` | New directory and plugin ID | A source skeleton exists; its default output has no visible UI |
| `pack` | Development directory and built Wasm | A restricted ZIP is created; an existing output is not overwritten |
| `check` | Plugin ZIP | The current local package and Wasm ABI checks pass |
| `run` | Wasm and line-delimited HostRequest JSON | ABI debugging; the caller still provides broker replies |
| `watch` | Wasm file | Reloads the instance after the file changes; does not compile source |

## Package limits

| Item | Current limit or rule |
| --- | --- |
| ZIP size | At most 64 MiB |
| File count | At most 512 |
| Single file | At most 32 MiB |
| Expanded total | At most 128 MiB |
| Manifest size | At most 64 KiB |
| Paths | Only the two root files and files under `assets/`; absolute paths, `..` traversal, backslashes, NUL, symlinks and case collisions are rejected |
| Archive entries | Encrypted entries, unknown compression and duplicate/conflicting entries are rejected |
| Runtime | The current ABI does not provide WASI, native libraries or postinstall |

These are package validation bounds. Runtime API calls, UI documents and resources have their own limits.

## Install and verify an update

1. Import the ZIP in NoriShell's plugin manager. Review its name, version, publisher description and requested permissions.
2. Approve the capabilities actually needed through the host flow, enable the plugin, then open it and perform the main action.
3. Check refusal, cancellation and disable behavior, including UI and resource cleanup.
4. For updates, retain the plugin identity and increment the version. Rebuild, check and import the candidate. Changes to content or permissions go through host validation again; do not reuse handles from the old package.

`pack` and `check` do not install, authorize or publish a plugin. Before distributing it, test your candidate in the target application and platforms. See [Errors and troubleshooting](./errors.en.md).
