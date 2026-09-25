<p align="center">
  <img src="src/assets/branding/norishell-app-icon.png" width="96" alt="NoriShell" />
</p>
<h1 align="center">NoriShell</h1>
<p align="center">A local-first terminal and remote connection workspace</p>
<p align="center">SSH · Local terminal · SFTP · Port forwarding · RDP / VNC · Encrypted Vault · Plugins</p>
<p align="center"><a href="LICENSE">GPL-3.0-only</a> · macOS / Windows</p>
<p align="center"><a href="README.md">简体中文</a> · English</p>

NoriShell brings terminal sessions, remote connections, file transfer, remote desktops, and credential management into one desktop application. Core connectivity requires no account. Host configuration stays local, while persisted passwords and private keys are stored in a separate encrypted Vault. End-to-end encrypted synchronization can be enabled selectively through the Norixor plugin or the [self-hosted sync example](example/self-host-sync/readme.md) when cross-device access is needed.

<p align="center">
  <a href="https://github.com/Norixor/NoriShell/releases/latest"><strong>Download NoriShell</strong></a>
  · <a href="#installation-and-quick-start">Installation guide</a>
</p>

## Contents

- [Highlights](#highlights)
- [Preview](#preview)
- [Features](#features)
- [Installation and quick start](#installation-and-quick-start)
- [Development from source](#development-from-source)
- [Security](#security)
- [Plugin development](#plugin-development)
- [Documentation](#documentation)
- [Troubleshooting](#troubleshooting)
- [Project layout](#project-layout)
- [License](#license)
- [Star History](#star-history)

## Highlights

- **Remote work in one window.** Switch between terminals, hosts, file transfers, tunnels, server monitoring, and remote desktops.
- **No account required.** Host settings stay on your device, and saved passwords and private keys go into the encrypted Vault.
- **Independent connections.** Closing a terminal, file transfer, tunnel, or monitor does not disconnect the others.
- **Extend when needed.** Install or upgrade plugins from local ZIP files. NoriShell supports macOS Apple Silicon / Intel and Windows x64 / ARM64.

For sync across devices, follow the [self-hosted sync guide](docs/guides/users/self-host-sync.en.md) to deploy the server and import the plugin. Upload from the first device, then select “Sync now” on the others. Restoring existing encrypted data requires that first device's Vault password.

## Preview

![NoriShell tabbed and split terminals](.github/assets/screenshots/terminal.png)

## Features

| Capability | Description |
| --- | --- |
| SSH and local terminals | Tabs, nested splits, search, reconnect, recent connections, and independent local Shell / PTY sessions |
| Host and connection management | Groups, tags, favorites, OpenSSH import, password/key/Agent authentication, and server fingerprint confirmation |
| Connection routes | Direct connections, HTTP CONNECT / SOCKS5 proxies, jump hosts, and per-host algorithm settings |
| Files and networking | Multi-pane SFTP browsing, transfer, preview/edit, live follow, plus local/remote/dynamic port forwarding |
| Remote desktops | RDP / VNC profiles and sessions, SSH gateways, scaling, input, text clipboard, and RDP audio capabilities |
| Server overview | Connection status for each host, with optional CPU, memory, network, and disk monitoring |
| Credential protection | Encrypted Vault, temporary credentials, and server fingerprint confirmation |
| Daily workflow | Quick commands, custom shortcuts, keyword highlighting, notifications, tray panel, and settings import/export |
| Plugins and themes | Install or upgrade plugins from local ZIP files and enable themes as needed |
| Optional synchronization | Synchronizes portable SSH, remote-desktop configuration, and non-secret preferences through the Norixor plugin or a self-hosted service without uploading the local Vault file |
| Telnet | Independent Telnet sessions with a plaintext-transport warning before connecting |

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

Open **Settings → About** and click “Check for updates”. When a new version is available, you can choose the installer for your platform from [GitHub Releases](https://github.com/Norixor/NoriShell/releases). For stable releases, the macOS app and installed Windows app can also download and install a signed update after you confirm. Windows portable ZIP builds and beta updates require a manual download. Beta builds can discover newer betas and stable releases; stable builds only offer stable releases.

### First connection

1. From the Terminal empty state, choose **Quick Connect**, add a Host, or import an OpenSSH configuration.
2. Review the destination, port, route, and authentication method.
3. On first use, verify the server host-key algorithm and fingerprint. Stop and investigate if a trusted key changes.
4. Create or unlock the Vault when a password, private key, or passphrase must be persisted.
5. After connecting, open Terminal, SFTP, Tunnel, or Overview resources as needed. These resources remain independent.

## Development from source

### Prerequisites

NoriShell uses Tauri 2, Vue 3, TypeScript, xterm, and Rust.

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

## Security

- Saved passwords and private keys are kept in the encrypted Vault. SSH connections require fingerprint confirmation on first use and are blocked if a trusted fingerprint changes.
- Plugins need your approval for their capabilities and cannot read the Vault. Sync does not upload the local Vault file.

Report security issues privately through the channels in [SECURITY.md](SECURITY.md).

## Plugin development

Plugins use WebAssembly and can request access to networking, storage, tasks, and UI capabilities. Install and upgrade them by importing a local ZIP file.

Plugin development entry points:

- [Complete plugin developer guide](docs/guides/developers/README.en.md)
- [Build your first plugin](docs/guides/developers/start/quickstart.en.md)
- [Call APIs and request permissions](docs/guides/developers/development/calling-api.en.md)
- [Build plugin interfaces](docs/guides/developers/development/ui.en.md)
- [Manage tasks and resources](docs/guides/developers/development/resources.en.md)
- [Package, install, and upgrade](docs/guides/developers/development/packaging.en.md)
- [API methods and types](docs/guides/plugin-api/README.en.md)
- [Example walkthroughs and source](docs/guides/developers/examples/README.en.md)
- [Self-hosted sync plugin and Go server](example/self-host-sync/readme.md)

More topics: [Wasm ABI](docs/guides/developers/wasm-abi.en.md) · [Theme plugins](docs/guides/developers/themes.en.md) · [Security and release checks](docs/guides/developers/security.en.md) · [SDK tooling](examples/plugins/sdk-tooling/README.md)

## Documentation

| Documentation index | Contents |
| --- | --- |
| [Complete documentation index](docs/guides/README.md) | English and Chinese entry points |
| [User guides](docs/guides/users/README.en.md) | Plugin import, permissions, recovery, and appearance |
| [Self-hosted sync installation](docs/guides/users/self-host-sync.en.md) | Server setup, plugin import, and first sync |
| [Developer guides](docs/guides/developers/README.en.md) | Package authoring, ABI, brokers, UI, themes, and security checks |
| [Plugin API reference](docs/guides/plugin-api/README.en.md) | Guest-accessible protocols, types, capabilities, resources, and events |
| [Core API catalog](docs/guides/core-api/README.en.md) | Internal desktop commands, events, and types |

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

## Project layout

```text
src/                    Vue UI, state projections, and shared components
src-tauri/              Tauri desktop entry point, native windows, and host integration
crates/                 Rust Core, protocols, Vault, persistence, and plugin capabilities
docs/guides/            User, plugin development, Plugin API, and Core API documentation
examples/plugins/       Wasm plugin and SDK examples
example/self-host-sync/ Self-hosted sync plugin, Go server, and Release packager
examples/theme-plugins/ Declarative theme examples
vendor/                 Patched dependencies with retained upstream licenses and change notes
```

See [vendor/README.md](vendor/README.md) for the origin, licensing, and modification scope of vendored patches.

## License

Original NoriShell code is licensed under the **GNU General Public License v3.0 only (GPL-3.0-only)**. See [LICENSE](LICENSE) for the complete terms.

Third-party source code, dependencies, and assets retain their respective licenses. See [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

## Star History

<a href="https://www.star-history.com/?repos=norixor%2Fnorishell&type=date&legend=top-left">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=norixor/norishell&type=date&theme=dark&legend=top-left" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=norixor/norishell&type=date&legend=top-left" />
    <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=norixor/norishell&type=date&legend=top-left" />
  </picture>
</a>
