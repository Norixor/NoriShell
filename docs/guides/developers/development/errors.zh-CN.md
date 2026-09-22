# 错误处理与排查

API 失败以 `PluginApiOutcome::Failed { code }` 返回稳定错误码。把它作为用户流程的一部分处理：保留必要状态，解释下一步，不用空白界面或假成功代替失败。

## 错误码速查

| 错误码 | 含义 | 建议处理 |
| --- | --- | --- |
| `invalidRequest` | 请求字段、格式或上下文不合法 | 对照方法参数表与实际 enum，检查 `kind`、字段名、callId 和大小限制 |
| `permissionDenied` | 没有所需权限或操作未获准 | 显示失败，检查 manifest 与 capability；由用户决定是否重新授权 |
| `interactionRequired` | 必须由用户参与的宿主操作 | 显示明确的继续按钮；不要在后台循环请求弹窗 |
| `vaultMissing` | 当前 Vault 尚未建立 | 显示状态，交给宿主引导处理，插件不创建凭据库 |
| `vaultLocked` | Vault 已锁定 | 用户明确点击后由宿主解锁；不在插件内收集密码 |
| `vaultRequiresReload` | Vault 状态需要重新加载 | 让宿主恢复状态后重新开始明确操作，不按“未建立”处理 |
| `unsupported` | 该请求或能力不受支持 | 查询 `describe` 并禁用不支持的功能，不猜测兼容接口 |
| `revoked` | 权限或资源资格已撤销 | 停止操作、清除旧 handle；不能用已记住的字符串重新获得权限 |
| `notFound` | 目标记录或资源不存在 | 刷新目标列表，不继续使用旧引用 |
| `conflict` | revision 或当前状态冲突 | 重新读取最新状态；让用户确认需要重新提交的修改 |
| `busy` | 资源或处理过程正忙 | 保持可见等待状态或让用户稍后重试，避免无限并发重发 |
| `quotaExceeded` | 超出运行时配额 | 减小请求、分块处理或释放无用资源；参考 `describe.limits` |
| `timedOut` | 操作超时 | 显示超时并核实副作用是否发生；不要无条件重试写操作 |
| `cancelled` | 操作已取消 | 结束等待、清理当前状态和资源 |
| `outcomeUnknown` | 无法确定请求是否产生副作用 | 先核对真实结果，再决定是否重试 |
| `cleanupIncomplete` | 资源清理未完全完成 | 显示未完成状态并查询当前资源，不能宣称全部释放 |
| `unavailable` | 当前服务或资源不可用 | 保留可恢复的用户输入，显示不可用并提供明确重试操作 |

这是错误码全集，不表示每个方法都会返回所有错误。方法页会补充与该操作相关的权限、结果与资源处理要求。

## 从现象找出问题

| 现象 | 优先检查 | 相关文档 |
| --- | --- | --- |
| 构建脚本找不到依赖或 Wasm target | Rust 工具链、target 和 Cargo 缓存；这不是运行时权限错误 | [快速开始](../start/quickstart.zh-CN.md) |
| ZIP 被拒绝 | 必填 manifest、协议 1.13、允许的包路径、文件大小与平台 | [打包与安装](./packaging.zh-CN.md) |
| 插件启用后没有可见界面 | `initialize` 是否返回有效 `ui.document`，target 是否注册，capability 是否批准 | [声明式界面](./ui.zh-CN.md) |
| 点击后界面消失或不更新 | 普通 `uiAction` 是否返回同 target 的完整 document；异步调用是否在 `brokerResult` 更新 | [调用与回调](./calling-api.zh-CN.md) |
| 请求发出但对应结果丢失 | callId 与 requestId 的角色、`result.kind`、reply 的 tagged union | [调用与回调](./calling-api.zh-CN.md) |
| 取得网络/进程句柄却没有结果 | 资源事件、HTTP 状态、退出码、关闭与背压 | [资源与权限](./resources.zh-CN.md) |
| 更新设置时反复冲突 | 是否重复使用旧 revision | [storage](../../plugin-api/storage.zh-CN.md) |
| 重启后任务中断 | 持久任务状态与进程内 payload 的区别，是否错误地自动重放旧请求 | [任务示例](../examples/workflow.zh-CN.md) |

## 验证自己的插件

| 检查阶段 | 要看到的结果 | 尚不能证明什么 |
| --- | --- | --- |
| Rust 编译 | 类型与调用参数正确 | 并未调用实际 Core 功能 |
| SDK `check` / ABI harness | 包格式与 Wasm ABI 可被当前 runtime 处理 | 不等同于权限批准或桌面可见结果 |
| 应用内安装、启用并操作 | 正确的界面、真实结果与错误反馈 | 一个环境的成功不自动证明所有平台和设备可用 |
| 拒绝、撤销、取消、重启 | 操作停止、状态可理解、资源被处理 | 不应以“没有崩溃”代替结果检查 |

排查日志记录方法、callId 和稳定错误码即可；避免输出密码、认证头、用户输入内容或完整上游响应。涉及文件写入、进程、网络副作用时，先核实结果再重试。
