# Service Demo：经审批的 HTTPS workflow

Service Demo 通过 Core 从 GitHub 查询 `rust-lang/rust` 的公开元数据。Wasm guest 没有 HTTP client、socket、proxy、凭据、文件系统或 Tauri 访问。只有用户点击 **Query GitHub** 后，它才会启动 Core 拥有的 workflow。

## 用户实际操作

1. 构建、检查、导入，批准 `uiPanel` 与 `networkDomain`，再启用包。
2. 打开 **Service demo**，点击 **Query GitHub**。这会创建 task，不会静默授予网络访问。
3. 点击 **Refresh** 直到 task 需要操作，再点击 **Continue query**。精确网络审批由 Core 展示，不属于 guest。
4. 完成后刷新，显示 HTTP status 与已验证的仓库名称、描述、star 数。task 活跃时可点 **Cancel query**，检查 revision 约束的取消。

guest 只接受 200 JSON response，将累计 body 限制为 64 KiB，验证 UTF-8 和预期字段，响应数据只留在存活 Wasm 实例中；不会写入插件 storage、task persistence 或日志。下面 Rust 行只是概念性节选，不是可编译源码：真实 `NetworkStart` 需要源码示例中的 SDK endpoint 与 HTTP request DTO 参数。

```rust
// 关键请求是类型化的；Core 解析并审阅 endpoint。
PluginApiOperation::NetworkStart { /* endpoint 与 HTTP request DTO */ }
```

结果必须区分：`needsUserAction` 时渲染明确的 **Continue query** 控件；task 完成后刷新并显示已验证 response；失败或取消时显示该状态。返回的 resource handle 只代表 Core 本地接受，并不证明远端服务器已经完成请求。

| 看到的现象 | 所需输入与先检查项 |
| --- | --- |
| **Continue query** 一直不可用 | 先启动 task，再刷新，直到 Core 返回需要显式用户动作的 task 状态。 |
| 出现提示框 | 所需输入是用户对精确 GitHub HTTPS 操作的审批；guest 不能收集凭据。 |
| task 完成却没有仓库数据 | 刷新 task 结果；只有完整的 200 JSON response 与预期字段才会渲染报告。 |
| 取消返回 `conflict` | 刷新当前 task snapshot，再用展示的 revision 发出新的取消请求。 |

## 构建边界

在 NoriShell 源码检出根目录运行：

```sh
python3 examples/plugins/service-demo/build.py --output /tmp/NoriShell-Service-Demo-1.0.1.zip
```

脚本会运行 guest 测试、用仓库工具链构建 Wasm、调用 SDK deterministic `pack`、运行 SDK `check` 并写入校验和 sidecar。输出文件名必须与上面一致，已存在时会拒绝覆盖。这些证据不能代替真实桌面审批、GitHub HTTPS、取消和资源清理验收。

适配前请看 [调用 Core](../development/calling-api.zh-CN.md) 的类型化网络操作、[错误](../development/errors.zh-CN.md) 的 `interactionRequired` 与 task 状态，以及 [打包](../development/packaging.zh-CN.md)。
