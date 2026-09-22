# 快速开始：第一个可见插件

这里先从仓库内的 **API Demo** 开始，因为它会创建一个可见的宿主渲染对话框。SDK 的 `scaffold` 命令适合下一步使用，但默认的 `example.ready` 只是 ABI 骨架，并不是可见应用贡献。

## 构建前准备

本手册对应 NoriShell `0.1.0-beta`。API Demo 源码的 `examples/plugins/api-demo/manifest.json` 目前仍是 `"minimumAppVersion": "0.1.0"`；运行构建命令前，请在自己的本地副本把它改为 `"minimumAppVersion": "0.1.0-beta"`，否则 beta 宿主会拒绝该 ZIP。

开发机器需要具备以下条件：

| 条件 | 原因 |
| --- | --- |
| 一份包含 `crates/plugin-sdk` 与 `examples/plugins` 的本地 NoriShell 源码检出 | SDK 当前随源码树交付，并且是该检出中的 source dependency。开始前请取得包含这两个目录的源码版本。 |
| Python 3 | API Demo 使用 Python 构建脚本。 |
| `rustup` 的 `1.97.1` 工具链与 `wasm32-unknown-unknown` | 示例将 Rust `cdylib` 编译到该 Wasm target。 |
| 已缓存的 Cargo 依赖 | API Demo 脚本明确使用 `--locked --offline`；本地缓存不完整时会停止。 |
| 匹配的 NoriShell 桌面应用 | 最后的导入、审批、启用与可见结果检查都需要它。 |

下面命令都在**本地 NoriShell 源码检出根目录**执行。仓库的 `rust-toolchain.toml` 选择 `1.97.1`；API Demo 构建脚本也会明确调用该工具链、以 `wasm32-unknown-unknown` 为 target，并为 Demo 构建和 SDK verifier 使用 `--locked --offline`。

```sh
# 查看本机已安装的工具链与 Wasm target。
rustup toolchain list
rustup target list --toolchain 1.97.1 --installed

# 若上面的检查缺少对应项，安装精确前置条件。
rustup toolchain install 1.97.1
rustup target add --toolchain 1.97.1 wasm32-unknown-unknown

# 在可联网时一次性填充受 lock 约束的工作区依赖缓存。
rustup run 1.97.1 cargo fetch --locked
```

`cargo fetch --locked` 在根工作区执行；该工作区包含 `crates/plugin-sdk` 和 verifier 使用的 `plugin-platform` 依赖。请先成功执行它，再进行离线构建；不要给 fetch 命令追加 `--offline`。构建 scaffold 项目时，这份检出必须保留可访问，因为 `scaffold` 会把本机 `norishell-plugin-sdk` 的绝对路径写入依赖。

然后确认 CLI 并构建第一个包：

```sh
# 确认当前检出中的 SDK 开发 CLI 可用。
CARGO_INCREMENTAL=0 cargo run --locked -p norishell-plugin-sdk --bin norishell-plugin-dev -- --help

# 构建真实的可见示例包；脚本也会写入 .sha256 sidecar。
python3 examples/plugins/api-demo/build.py --output /tmp/NoriShell-API-Demo-1.0.1.zip

# 用 SDK 包校验器和生产 Wasm ABI runtime 检查 ZIP。
CARGO_INCREMENTAL=0 cargo run --locked -p norishell-plugin-sdk --bin norishell-plugin-dev -- \
  check /tmp/NoriShell-API-Demo-1.0.1.zip
```

构建脚本会编译 guest Wasm 并运行 API Demo ABI verifier。`check` 还会打开受限 ZIP，校验 manifest、资源，并以生产 runtime 加载 `plugin.wasm`。这两个命令都不会安装包、批准 capability，也不会连接真实 Core broker。

## 在 NoriShell 中看到结果

1. 在匹配的 NoriShell 桌面应用打开 **插件 → 导入 ZIP**，选择 `/tmp/NoriShell-API-Demo-1.0.1.zip`。
2. 阅读包身份与申请的 capabilities。API Demo 申请 `uiPanel` 和 `clipboardWrite`。
3. 完成本地导入，在宿主流程中显式批准所申请能力，并启用插件。
4. 打开 **API demo**，点击 **Query API**。对话框应把初始提示替换成当前 Core 返回的插件方法和限制。
5. 点击 **Copy API description**，对照剪贴板文本与展示结果。禁用插件后，确认其贡献入口消失。

打开对话框本身不会调用 API。若 ZIP 能导入却没有界面，请先检查插件 `ui.document` 贡献及其 target，不能把“已启用”当作用户流程已经成功。请继续看 [UI document](../development/ui.zh-CN.md) 与 [API Demo 往返](../examples/api-demo.zh-CN.md)。

## 演示完成后创建自己的项目

本地 CLI 会创建一个空的、不会覆盖目标目录的 Rust/Wasm 项目。在 NoriShell 源码检出根目录运行：

```sh
CARGO_INCREMENTAL=0 cargo run --locked -p norishell-plugin-sdk --bin norishell-plugin-dev -- \
  scaffold /tmp/norishell-example --id com.example.norishell-example --name "My NoriShell plugin"
```

它会写入 `Cargo.toml`、`manifest.json`、`src/lib.rs` 和 README。生成的 Cargo manifest 指向当前检出的绝对 SDK 路径；迁移项目时请保留该检出，或有意识地更新依赖路径。生成的 guest 只输出 `example.ready`，因此在期待出现 UI 前，需要按 API Demo 的 `Initialize` → `ui.document` 模式补上有效 document。

构建生成项目、打包并检查新 ZIP。`pack` 不会覆盖已存在的输出文件。

```sh
RUSTC="$(rustup which --toolchain 1.97.1 rustc)" CARGO_INCREMENTAL=0 cargo build --release \
  --target wasm32-unknown-unknown --target-dir /tmp/norishell-example-target \
  --manifest-path /tmp/norishell-example/Cargo.toml

CARGO_INCREMENTAL=0 cargo run --locked -p norishell-plugin-sdk --bin norishell-plugin-dev -- \
  pack /tmp/norishell-example \
  --wasm /tmp/norishell-example-target/wasm32-unknown-unknown/release/norishell_com_example_norishell_example.wasm \
  --output /tmp/norishell-example-0.1.0.zip

CARGO_INCREMENTAL=0 cargo run --locked -p norishell-plugin-sdk --bin norishell-plugin-dev -- \
  check /tmp/norishell-example-0.1.0.zip
```

不要原地修改 API Demo 后仍以其原身份安装无关实验。请给自己的包设置独立 `pluginId`、名称、版本、实际所需 capability、动作和静态资源。下一步阅读 [语言边界](./languages.zh-CN.md)、[调用 Core](../development/calling-api.zh-CN.md) 与 [打包](../development/packaging.zh-CN.md)。
