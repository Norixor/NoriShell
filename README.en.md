<p align="center">
  <img src="src/assets/branding/norishell-app-icon.png" width="96" alt="NoriShell" />
</p>
<h1 align="center">NoriShell</h1>
<p align="center">A local-first terminal and remote connection workspace</p>
<p align="center">SSH · Local terminal · SFTP · Port forwarding · RDP / VNC · Encrypted Vault · Plugins</p>
<p align="center"><a href="LICENSE">GPL-3.0-only</a> · macOS / Windows · v0.1.0-beta</p>
<p align="center"><a href="README.md">简体中文</a> · English</p>

NoriShell brings terminal sessions, remote connections, file transfer, remote desktops, and credential management into one desktop application. Core connectivity requires no account. Host configuration stays local, while persisted passwords and private keys are stored in a separate encrypted Vault. End-to-end encrypted synchronization can be enabled selectively through the Norixor plugin when cross-device access is needed.

The application is built with **Tauri 2, Vue 3, TypeScript, xterm, and Rust**. Rust Core owns connections, secrets, persistence, and resource lifecycles. The frontend owns interaction and reconstructible state projections.

## Contents

- [Highlights](#highlights)
- [Preview](#preview)
- [Features](#features)
- [Installation and quick start](#installation-and-quick-start)
- [Development from source](#development-from-source)
- [Local-first and security boundaries](#local-first-and-security-boundaries)
- [Plugin development](#plugin-development)
- [Documentation](#documentation)
- [Troubleshooting](#troubleshooting)
- [Architecture and project layout](#architecture-and-project-layout)
- [License](#license)

## Highlights

- **One workspace for remote operations.** Switch between terminals, hosts, file transfers, tunnels, server monitoring, and remote desktops in one window.
- **Local-first operation.** SSH, local terminals, SFTP, tunnels, and local configuration do not depend on a cloud account.
- **Explicit identity and secret boundaries.** First-use SSH fingerprints require confirmation, key changes block the connection, and persisted secrets belong in the encrypted Vault.
- **Independent connections.** Terminals, file transfers, tunnels, and monitoring run independently; closing one does not disconnect the others.
- **Constrained plugins.** Plugins use an isolated WebAssembly host, fine-grained capabilities, and protected approval. Installation and upgrades require an explicit local ZIP import.
- **Built for macOS and Windows.** Installers are available for macOS Apple Silicon / Intel, Windows x64, and Windows ARM64.

## Preview

![NoriShell tabbed and split terminals](.github/assets/screenshots/terminal.png)

## Features

| Capability | Description |
| --- | --- |
| SSH and local terminals | Tabs, nested splits, search, reconnect, recent connections, and independent local Shell / PTY sessions |
| Host and connection management | Groups, tags, favorites, non-secret OpenSSH import, password/key/Agent authentication, and server fingerprint verification |
| Routes and compatibility policy | Direct, HTTP CONNECT, SOCKS5, Jump Host / Host Chain, and Host-scoped algorithm exceptions |
| Files and networking | Multi-pane SFTP browsing, transfer, preview/edit, live follow, plus Local / Remote / Dynamic port forwarding |
| Remote desktops | RDP / VNC profiles and sessions, SSH gateways, scaling, input, text clipboard, and RDP audio capabilities |
| Server overview | Per-Host aggregation of Terminal, SFTP, Tunnel, and Metrics resources, with optional CPU, memory, network, and disk metrics |
| Credential protection | Separate encrypted Vault, one-time credentials, protected windows, and strict host-key confirmation and mismatch blocking |
| Daily workflow | Quick commands, custom shortcuts, keyword highlighting, notifications, tray panel, and settings import/export |
| Plugins and themes | Isolated WebAssembly plugins, fine-grained permissions, local ZIP import/upgrade, and data-only declarative themes |
| Optional synchronization | Synchronizes explicitly selected encrypted SSH and remote-desktop configuration through the Norixor plugin without uploading the local Vault file |
| Telnet | Independent Telnet sessions gated by acknowledgement of plaintext transport, missing server identity, and tampering risks |

## Installation and quick start

Visit [GitHub Releases](https://github.com/Norixor/NoriShell/releases) and choose an installer or a standalone ZIP for your device.

| Platform | Architecture | Installer | Standalone ZIP |
| --- | --- | --- | --- |
| macOS 13 or later | Apple Silicon (ARM64) | `.dmg` | Extract and open `NoriShell.app` |
| macOS 13 or later | Intel (x64) | `.dmg` | Extract and open `NoriShell.app` |
| Windows | x64 (Intel / AMD) | `-setup.exe` | Extract the entire folder and run `norishell.exe` |
| Windows | ARM64 | `-setup.exe` | Extract the entire folder and run `norishell.exe` |

For a macOS installer, drag NoriShell into Applications; on Windows, follow the installer. ZIP builds use the same local settings and data directories. Extract the full archive before running. The Windows ZIP requires WebView2 Runtime to be installed.

### Check for updates

Open **Settings → About** and click “Check for updates”. NoriShell reads public version information from [GitHub Releases](https://github.com/Norixor/NoriShell/releases). When an update is available, “Download on GitHub” opens the release page in your browser so you can choose the installer for your platform and architecture.

Download and install updates manually. Beta builds can discover newer betas and stable releases; stable builds only offer stable releases.

### First connection

1. From the Terminal empty state, choose **Quick Connect**, add a Host, or import an OpenSSH configuration.
2. Review the destination, port, route, and authentication method.
3. On first use, verify the server host-key algorithm and fingerprint. Stop and investigate if a trusted key changes.
4. Create or unlock the Vault when a password, private key, or passphrase must be persisted.
5. After connecting, open Terminal, SFTP, Tunnel, or Overview resources as needed. These resources remain independent.

## Development from source

### Prerequisites

- Node.js **22 or newer**.
- pnpm **10.32.1**, matching `packageManager` in `package.json`.
- Rust **1.97.1**, pinned by `rust-toolchain.toml`, with `rustfmt` and `clippy`.
- [Tauri 2 platform prerequisites](https://v2.tauri.app/start/prerequisites/): Xcode Command Line Tools on macOS; MSVC C++ build tools and WebView2 on Windows.

### Run locally

```sh
git clone https://github.com/Norixor/NoriShell.git
cd NoriShell
pnpm install --frozen-lockfile
pnpm tauri dev
```

### Checks and builds

```sh
# Generated contract consistency, types, lint, frontend tests, and production build
pnpm check

# Rust formatting, tests, and strict static analysis
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings

# Build the desktop application for the current platform
pnpm tauri build
```

## Local-first and security boundaries

- **Secrets belong in the Vault.** Persisted passwords, private keys, and passphrases are not stored as ordinary SQLite configuration or written to logs.
- **Server identity is checked before connecting.** A first-use SSH fingerprint requires explicit confirmation, and a changed trusted key blocks the connection.
- **Resources own independent connections.** Terminal, SFTP, Tunnel, and Metrics resources do not share an SSH transport.
- **Plugins start without authority.** Guest code cannot directly access the Vault, SQLite, SSH sockets, host DOM, or arbitrary Tauri commands.
- **Sensitive decisions use protected windows.** Vault, credential, host-key, plugin-permission, and other security decisions are isolated from ordinary plugin UI.
- **Users choose synchronization scope.** The sync service does not receive Vault passwords, KEKs, VMKs, Known Hosts, or device auto-unlock material.

Report security issues privately through the channels in [SECURITY.md](SECURITY.md).

## Plugin development

Plugins use a versioned, constrained WebAssembly ABI. They request networking, storage, task, UI, or protocol resources through typed host brokers. Declaring a capability only requests permission; it cannot bypass user approval, resource scope, generation fences, or host lifecycle rules.

NoriShell has no online plugin marketplace. To install or upgrade a plugin, the user explicitly selects a local ZIP. A newer version of the same plugin follows package validation, fresh permission review, anti-rollback checks, and atomic replacement.

Plugin development entry points:

- [Complete plugin developer guide](docs/guides/developers/README.en.md)
- [Build your first plugin](docs/guides/developers/start/quickstart.en.md)
- [Call APIs and request permissions](docs/guides/developers/development/calling-api.en.md)
- [Build plugin interfaces](docs/guides/developers/development/ui.en.md)
- [Manage tasks and resources](docs/guides/developers/development/resources.en.md)
- [Package, install, and upgrade](docs/guides/developers/development/packaging.en.md)
- [API methods and types](docs/guides/plugin-api/README.en.md)
- [Example walkthroughs and source](docs/guides/developers/examples/README.en.md)

More topics: [Wasm ABI](docs/guides/developers/wasm-abi.en.md) · [Theme plugins](docs/guides/developers/themes.en.md) · [Security and release checks](docs/guides/developers/security.en.md) · [SDK tooling](examples/plugins/sdk-tooling/README.md)

## Documentation

| Documentation index | Audience and boundary |
| --- | --- |
| [Complete documentation index](docs/guides/README.md) | English and Chinese entry points |
| [User guides](docs/guides/users/README.en.md) | Plugin import, permissions, recovery, and appearance |
| [Developer guides](docs/guides/developers/README.en.md) | Package authoring, ABI, brokers, UI, themes, and security checks |
| [Plugin API reference](docs/guides/plugin-api/README.en.md) | Guest-accessible protocols, types, capabilities, resources, and events |
| [Core API catalog](docs/guides/core-api/README.en.md) | Renderer IPC, commands, events, handlers, and Plugin Host boundaries; not a plugin permission surface |

## Troubleshooting

### macOS reports that the app is damaged

If macOS blocks an unsigned app or an app with a remaining quarantine attribute, run these commands in Terminal:

```sh
sudo spctl --master-disable
sudo xattr -r -d com.apple.quarantine "/Applications/NoriShell.app"
```

Then open NoriShell again.

### Local build environment differs

Use the pnpm and Rust versions pinned by the repository. If dependencies are inconsistent, run `pnpm install --frozen-lockfile` again. On macOS, also verify the Xcode Command Line Tools and license state before the first build.

## Architecture and project layout

```text
Vue Desktop UI
    │ typed Tauri commands / channels / events
Rust Core
    ├── SSH, PTY, SFTP, Tunnel, Metrics, and remote-desktop lifecycles
    ├── SQLite non-secret metadata + separate encrypted Vault
    └── Plugin package validation, capability broker, and resource fences
        │ versioned plugin protocol
Isolated Plugin Host
    └── One constrained Wasm instance with no direct Vault, SQLite, socket, or Tauri access
```

```text
src/                    Vue UI, state projections, and shared components
src-tauri/              Tauri desktop entry point, native windows, and host integration
crates/                 Rust Core, protocols, Vault, persistence, and plugin capabilities
docs/guides/            User, plugin development, Plugin API, and Core API documentation
examples/plugins/       Wasm plugin and SDK examples
examples/theme-plugins/ Declarative theme examples
vendor/                 Patched dependencies with retained upstream licenses and change notes
```

See [vendor/README.md](vendor/README.md) for the origin, licensing, and modification scope of vendored patches.

## License

Original NoriShell code is licensed under the **GNU General Public License v3.0 only (GPL-3.0-only)**. See [LICENSE](LICENSE) for the complete terms. Distribution of a GPL-covered version must satisfy the corresponding source-availability and other license obligations.

Third-party source code, dependencies, and assets retain their respective licenses. See [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
