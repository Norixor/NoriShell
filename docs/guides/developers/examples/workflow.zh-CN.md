# Workflow Demo：等待 Core，不在 Wasm 中 sleep

Workflow Demo 声明三步 task：查看 API、创建三秒 timer、Core 交付 timer event 后再次查看 API。timer 请求与 event 之间，guest 返回 waiting workflow response；它不会在 Wasm 内 sleep 或 poll。下面顺序说明完整事件循环，不是可独立编译的 Rust 源码。

## 预期用户流程

导入并启用后，打开 **Workflow demo**，点击 **Start task**。点击 **Refresh tasks** 查看当前 Core task record。timer 等待期间刷新，然后点击 **Cancel task**，检查 scoped resource cleanup。取消会携带当前展示 task revision；`conflict` 表示 snapshot 已过期，需刷新后再操作。

Core 报告 `needsUserAction` 时，Demo 提供 revision 约束的恢复操作。本 task 本身不需要网络、文件或进程权限。

```text
UiAction(Start task) -> api.request(taskStart)
BrokerResult(task snapshot) -> ui.document
WorkflowEvent(timer) -> workflow response / 下一次类型化调用
BrokerResult -> 带新 snapshot 的 ui.document
```

只有 task 和 step metadata 会持久化。payload、reply 和 Wasm continuation 都留在内存中。应用重启时，Core 会把未完成 task 协调为 `interrupted`；它不会重放之前的 task dispatch。刷新可以展示该 record，重新开始则创建新操作。

| 看到的现象 | 所需输入与预期结果 |
| --- | --- |
| 点击 **Start task** 后没有 task | 动作需要可见且已启用的包和有效 `taskStart` request；之后刷新读取 Core task record。 |
| timer 看起来没有反应 | 等待 Core timer event 后刷新；guest 不拥有后台 sleep loop。 |
| 取消或恢复时出现 `conflict` | 先刷新；下一次用户点击必须使用最新 task revision。 |
| task 中途重启应用 | 预期为 `interrupted`，不是自动重放；用户要再次运行时新建 task。 |

## 构建与打包

在 NoriShell 源码检出根目录运行：

```sh
RUSTC="$(rustup which --toolchain 1.97.1 rustc)" CARGO_INCREMENTAL=0 cargo build \
  --manifest-path examples/plugins/workflow-demo/Cargo.toml \
  --target wasm32-unknown-unknown --release
cargo run -p norishell-plugin-sdk --bin norishell-plugin-dev -- \
  pack examples/plugins/workflow-demo \
  --wasm examples/plugins/workflow-demo/target/wasm32-unknown-unknown/release/norishell_workflow_demo.wasm \
  --output /tmp/NoriShell-Workflow-Demo-1.0.0.zip
cargo run -p norishell-plugin-sdk --bin norishell-plugin-dev -- \
  check /tmp/NoriShell-Workflow-Demo-1.0.0.zip
```

Wasm 构建与包检查只验证本地产物边界。导入、task 执行、取消和重启行为仍需在 NoriShell 中验证。适配 task 与 revision 处理时请看 [调用 Core](../development/calling-api.zh-CN.md) 和 [错误](../development/errors.zh-CN.md)。
