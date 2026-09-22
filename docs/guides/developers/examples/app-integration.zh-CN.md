# 应用集成：命令、选择器与不透明句柄

App Demo 演示插件经明确用户动作后可以请求的 Core 拥有应用集成：应用注册、带可选快捷键的命令、状态项、通知、受控导航和只读文件流程。

初始化只渲染 dialog。**Register app command** 请求 Core 注册 **Open text file**；该宿主命令映射到与 **Select text file** 相同的已声明动作。它不会创建操作系统文件关联；可选 Alt+Shift+O 快捷键必须由用户在宿主命令面板中启用。

选择文件流程特意分段。下面是事件流程节选，不是可独立编译的 Rust 源码：

```text
Select text file -> api.request(filePick) -> BrokerResult(rootHandle)
Read preview -> api.request(file read) -> BrokerResult(data)
Close file handle -> api.request(resourceClose) -> BrokerResult(closed)
```

原生选择器和精确文件范围审批属于 Core。guest 只收到不透明 root handle，第二次点击才读取一个有界 chunk，最多显示 2,048 个字符，并显式关闭 resource。第一个 handle 关闭前，第二次选择保持不可用。

| 看到的现象 | 所需输入与预期结果 |
| --- | --- |
| 没有 **Open text file** | 先点击 **Register app command**；宿主注册是显式动作，不属于初始化行为。 |
| 选择器要求审批 | 所需输入是用户对精确只读文件选择的审批。guest 只能接收返回的不透明 handle。 |
| **Read preview** 不可用 | 先选择一个文本文件；read result 后 dialog 最多展示 2,048 个字符。 |
| 第二次选择被阻止 | 显式关闭第一个文件 handle，再选择另一个文件。 |

在 NoriShell 源码检出根目录构建：

```sh
RUSTC="$(rustup which --toolchain 1.97.1 rustc)" CARGO_INCREMENTAL=0 cargo build \
  --manifest-path examples/plugins/app-demo/Cargo.toml --target wasm32-unknown-unknown --release
cargo run -p norishell-plugin-sdk --bin norishell-plugin-dev -- \
  pack examples/plugins/app-demo \
  --wasm examples/plugins/app-demo/target/wasm32-unknown-unknown/release/norishell_app_demo.wasm \
  --output /tmp/NoriShell-App-Demo-1.0.0.zip
cargo run -p norishell-plugin-sdk --bin norishell-plugin-dev -- check /tmp/NoriShell-App-Demo-1.0.0.zip
```

本地检查不证明原生选择器、审批、命令/快捷键、通知、导航或清理。复制此生命周期前请阅读 [调用 Core](../development/calling-api.zh-CN.md) 与 [错误](../development/errors.zh-CN.md)。
