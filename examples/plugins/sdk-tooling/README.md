# SDK 开发工具

`norishell-plugin-dev` 为本地 Rust Wasm 插件提供最小开发闭环，目标协议固定为当前唯一的 `1.13`。

```sh
# 在 NoriShell 仓库根目录构建工具
CARGO_INCREMENTAL=0 cargo run --locked -p norishell-plugin-sdk --bin norishell-plugin-dev -- --help

# 建立一个不会覆盖已有目录的插件骨架
CARGO_INCREMENTAL=0 cargo run --locked -p norishell-plugin-sdk --bin norishell-plugin-dev -- \
  scaffold /tmp/norishell-example --id com.example.norishell-example

# 以独立临时 target 编译插件，然后检查和生成稳定 ZIP
RUSTC="$(rustup which --toolchain 1.97.1 rustc)" CARGO_INCREMENTAL=0 cargo build --release --target wasm32-unknown-unknown \
  --target-dir /tmp/norishell-example-target --manifest-path /tmp/norishell-example/Cargo.toml
CARGO_INCREMENTAL=0 cargo run --locked -p norishell-plugin-sdk --bin norishell-plugin-dev -- \
  pack /tmp/norishell-example \
  --wasm /tmp/norishell-example-target/wasm32-unknown-unknown/release/norishell_com_example_norishell_example.wasm \
  --output /tmp/norishell-example-0.1.0.zip
CARGO_INCREMENTAL=0 cargo run --locked -p norishell-plugin-sdk --bin norishell-plugin-dev -- \
  check /tmp/norishell-example-0.1.0.zip
```

`check` 与 `pack` 复用 NoriShell 的本地 ZIP 包检查、设置 schema、协议 catalog 和生产 Wasm ABI runtime。它们不会把本地开发包当作已授予权限的生产插件。ZIP 的安装仍必须经过 NoriShell 的本地导入、哈希绑定和受保护权限流程。

运行时 harness 从 stdin 接收每行一个 `PluginHostRequest` JSON，并在 stdout 返回真实的 `PluginRuntimeOutput` JSON：

```sh
printf '%s\n' '{"protocolMajor":1,"protocolMinor":13,"requestId":"init-1","kind":"initialize","payloadJson":"{}"}' |
  CARGO_INCREMENTAL=0 cargo run --locked -p norishell-plugin-sdk --bin norishell-plugin-dev -- \
    run /tmp/norishell-example-target/wasm32-unknown-unknown/release/norishell_com_example_norishell_example.wasm --debug
```

如果插件返回 `api.request`，harness 只将该请求输出给测试调用方。测试调用方必须自己构造经过类型检查的 `BrokerResult` 再写回 stdin；未接入真实 Core 时不会虚构成功结果。`watch <wasm>` 只监视指定的 Wasm 文件，成功重载后会重新执行最后一个显式 `Initialize` 请求，新 runtime 不保留旧 Wasm 状态。

## Guest API 类型

SDK 直接导出可由 guest 构造或解析的当前 protocol 1.13 DTO：网络、受限本地文件、SFTP、插件凭据、插件私有存储、定时器与订阅、独立 remote exec、本机 process、串口和协议 provider。`PluginApiOperation`、`PluginApiReply`、资源事件、稳定错误、能力枚举与 `WireSequence` 也一并导出，因此 guest 不需要依赖 `norishell-core-api`。

这些类型只表示请求、非秘密结果和 Core 生成的不透明 handle；它们不授予能力。SDK 不导出冻结网络/进程/串口计划、Host scope、授权 lease 或 token、受保护审批 DTO、凭据原文与任何 SecretRef。安装和运行时权限仍由真实 Core 重新检查。

仓库内的编译门禁覆盖这些公开类型：

```sh
CARGO_INCREMENTAL=0 cargo check --offline -p norishell-plugin-sdk --example verify_guest_api_types
```
