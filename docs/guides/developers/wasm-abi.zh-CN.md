# Protocol 13 Wasm ABI

一个 Wasm 实例会在其 Plugin Host 实例存续期间常驻，因此有界 guest 状态可以留在插件对象中；但 guest memory 不能当作权限或持久存储。Core 会串行调用同一实例，也可在生命周期、配额或失败边界停止它。

模块导出线性 `memory`、`nvx_alloc(i32) -> i32`、`nvx_handle(i32, i32) -> i32` 和 `nvx_dealloc(i32, i32)`；**唯一** import 是 `norishell.host.emit(i32, i32) -> i32`。没有 WASI surface。Core 通过 `nvx_alloc` 分配请求字节，调用 `nvx_handle`，然后只用同一 allocation 调用一次 `nvx_dealloc`。非零 `nvx_handle` 状态代表 guest ABI 失败，不序列化私有细节。

请使用 `norishell_plugin_sdk::export_plugin!`，不要复制 ABI shim。SDK 会生成完整 export、唯一 host import、绑定 manifest 声明且受支持 minor 的 envelope，并把输出绑定到收到的 request id。

```rust
use norishell_plugin_sdk::{Plugin, PluginError, PluginHostRequest, PluginRuntimeOutput};
#[derive(Default)] struct Inspector;
impl Plugin for Inspector { fn handle(&mut self, _: PluginHostRequest) -> Result<Vec<PluginRuntimeOutput>, PluginError> { Ok(Vec::new()) } }
norishell_plugin_sdk::export_plugin!(Inspector);
```

可用输出见[Broker 调用](broker.zh-CN.md)。不得为旧 protocol minor 自造第二套 ABI。
