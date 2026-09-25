<!--
Release notes template. Replace APP_VERSION and SYNC_VERSION, remove empty change categories,
and describe only changes included in the tagged build. Keep download links bound to this tag.
Use RELEASING.md for the public release format; local build and verification steps live in .build.
-->

# NoriShell v{{APP_VERSION}}

## 中文

### 新增

- {{本版新增的用户可见能力；没有则删除本节}}

### 调整

- {{现有操作方式或体验的变化；没有则删除本节}}

### 修复

- {{具体场景、原有问题和修复后的表现；没有则删除本节}}

---

## English

### Added

- {{User-visible capability added in this release; remove this section if none}}

### Changed

- {{Change to an existing workflow or experience; remove this section if none}}

### Fixed

- {{Trigger, previous problem, and resulting behavior; remove this section if none}}

## Downloads / 下载

| Platform / 平台 | Installer / 安装包 | Complete app ZIP / 完整应用 ZIP |
| --- | --- | --- |
| macOS · Apple Silicon (ARM64) | [⬇ DMG](https://github.com/Norixor/NoriShell/releases/download/v{{APP_VERSION}}/NoriShell_{{APP_VERSION}}_macos_arm64.dmg) | [⬇ ZIP](https://github.com/Norixor/NoriShell/releases/download/v{{APP_VERSION}}/NoriShell_{{APP_VERSION}}_macos_arm64.zip) |
| macOS · Intel (x64) | [⬇ DMG](https://github.com/Norixor/NoriShell/releases/download/v{{APP_VERSION}}/NoriShell_{{APP_VERSION}}_macos_x64.dmg) | [⬇ ZIP](https://github.com/Norixor/NoriShell/releases/download/v{{APP_VERSION}}/NoriShell_{{APP_VERSION}}_macos_x64.zip) |
| Windows · x64 | [⬇ Setup](https://github.com/Norixor/NoriShell/releases/download/v{{APP_VERSION}}/NoriShell_{{APP_VERSION}}_windows_x64-setup.exe) | [⬇ ZIP](https://github.com/Norixor/NoriShell/releases/download/v{{APP_VERSION}}/NoriShell_{{APP_VERSION}}_windows_x64.zip) |
| Windows · ARM64 | [⬇ Setup](https://github.com/Norixor/NoriShell/releases/download/v{{APP_VERSION}}/NoriShell_{{APP_VERSION}}_windows_arm64-setup.exe) | [⬇ ZIP](https://github.com/Norixor/NoriShell/releases/download/v{{APP_VERSION}}/NoriShell_{{APP_VERSION}}_windows_arm64.zip) |

**Self-hosted Sync / 自建同步** · [Server ZIP / 服务端](https://github.com/Norixor/NoriShell/releases/download/v{{APP_VERSION}}/NoriShell_SelfHostSync_Server_{{SYNC_VERSION}}.zip) · [Plugin ZIP / 插件包](https://github.com/Norixor/NoriShell/releases/download/v{{APP_VERSION}}/NoriShell_SelfHostSync_Plugin_{{SYNC_VERSION}}.zip) · [Installation guide / 安装指南](https://github.com/Norixor/NoriShell/blob/v{{APP_VERSION}}/docs/guides/users/self-host-sync.en.md)

**Checksums / 校验** · [SHA256SUMS.txt](https://github.com/Norixor/NoriShell/releases/download/v{{APP_VERSION}}/SHA256SUMS.txt)

macOS ZIPs contain the complete `NoriShell.app`. Extract Windows ZIPs fully before running `norishell.exe`; Windows requires WebView2 Runtime.

macOS ZIP 包含完整的 `NoriShell.app`。Windows ZIP 请完整解压后运行 `norishell.exe`；系统需安装 WebView2 Runtime。
