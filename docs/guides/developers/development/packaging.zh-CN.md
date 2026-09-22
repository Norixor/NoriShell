# 打包、安装与更新

开发目录可以包含能产出 protocol 1.13 Wasm ABI 的任意语言源码、构建脚本和测试；交给 NoriShell 的插件 ZIP 只包含运行必需的 manifest、Wasm 和静态资源。当前 SDK 的 `pack` 与 `check` 是 Rust 工具；其他实现必须产出同一经校验的包边界。

## ZIP 目录结构

```text
my-plugin-0.1.0.zip
├── manifest.json
├── plugin.wasm
└── assets/
    └── isolated/
        └── toolbox.html
```

`manifest.json` 和 `plugin.wasm` 必须各有一份，并位于 ZIP 根目录。`assets/` 是可选的；使用 `surfaceId: "toolbox"` 打开隔离页面时，才需要对应的 `assets/isolated/toolbox.html`。不要把 Cargo 源码目录、原生动态库、`node_modules` 或安装脚本放进 ZIP。

## Manifest 字段表

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `pluginId` | string | 是 | 独立且稳定的插件标识，例如 `com.example.inspector`；不要复用官方示例身份 |
| `name` | string | 是 | 展示给用户的插件名称 |
| `publisher` | string | 是 | 发布者说明；本地包中的这段文字不是已验证的签名身份 |
| `version` | string | 是 | canonical semver，例如 `0.1.0`；更新包时使用新版本 |
| `protocolMajor` | integer | 是 | 当前为 `1` |
| `protocolMinor` | integer | 是 | 当前常驻 Wasm ABI 为 `13`。major 1 宿主支持从 `13` 到自身当前常驻 ABI minor 的稳定范围；本宿主当前接受 `13`。 |
| `platform` | string | 是 | 当前本地包为 `desktop` |
| `architectures` | string[] | 是 | 声明适用架构；通用 Wasm 示例使用 `["universal"]` |
| `capabilities` | string[] | 是 | 申请的能力，最多 32 项且不能重复；声明不等于批准 |
| `minimumAppVersion` | string | 是 | 最低适用应用版本；按实际使用的能力选择 |
| `publisherKeyId` | string 或 null | 否 | 发布者签名相关元数据，不应填入伪造值 |
| `publisherSignature` | string 或 null | 否 | 对应签名；字符串存在不等于本地导入已验证发布者 |
| `packageUrl` | string 或 null | 否 | 包来源元数据；本地开发包可省略，不会因此自动发布 |

下面示例可作为仅使用 UI 与插件存储的本地 manifest 起点。修改身份与版本后，再按实际功能调整 capabilities。

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

## 构建、打包和检查

先按[快速开始](../start/quickstart.zh-CN.md)创建 `/tmp/norishell-example`。以下命令在 NoriShell 源码根目录执行，输出文件名对应该示例身份。

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

| 命令 | 输入 | 成功表示什么 |
| --- | --- | --- |
| `scaffold` | 新目录与插件 ID | 已创建源码骨架；默认输出并不包含可见 UI |
| `pack` | 开发目录与编译后的 Wasm | 生成受限 ZIP；输出文件已存在时不会覆盖 |
| `check` | 插件 ZIP | 包与 Wasm ABI 通过当前本地校验 |
| `run` | Wasm 文件与逐行 HostRequest JSON | 调试 ABI；测试调用方仍须提供 broker 回复 |
| `watch` | Wasm 文件 | 文件变化后重载实例；不会自动编译源码 |

## 包限制

| 项目 | 当前限制或规则 |
| --- | --- |
| ZIP 大小 | 最多 64 MiB |
| 文件数 | 最多 512 |
| 单文件大小 | 最多 32 MiB |
| 解压后总大小 | 最多 128 MiB |
| Manifest 大小 | 最多 64 KiB |
| 路径 | 只允许根目录两项与 `assets/` 下文件；拒绝绝对路径、`..` 穿越、反斜杠、NUL、符号链接及大小写冲突 |
| 归档 | 拒绝加密条目、未知压缩方式和重复/冲突条目 |
| 运行环境 | 当前 ABI 不提供 WASI、原生库或 postinstall |

这些是包检查的上限；运行时 API、UI document 和资源仍有各自的限制。

## 安装并检查更新

1. 在 NoriShell 的插件管理中导入 ZIP，核对名称、版本、发布者说明与申请的权限。
2. 通过宿主流程批准实际需要的能力，启用后打开插件并执行核心动作。
3. 检查允许、拒绝、取消和禁用后的行为，确认界面与资源都能正确结束。
4. 更新时保持同一插件身份并提升版本，重新构建、检查和导入候选包。内容或权限变化应重新接受宿主核验，不应复用旧 package 的句柄。

`pack` 与 `check` 不会安装、授权或发布插件。面向用户交付前，需要在目标应用与平台实测自己的候选包；参见[错误与排查](./errors.zh-CN.md)。
