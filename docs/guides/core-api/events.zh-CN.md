# Core event 与 resource event

Core event 是 Core 持有状态的投影，绝不是 permission token。renderer listener 必须把每个 event 绑定到当前 resource/session/workspace generation，并在 detach、close、revoke 或 route replace 后丢弃迟到 event。完整源码检查点见[events.generated.md](events.generated.md)：2 个具名 event symbol 与 25 个导出的 event/output payload type。

| Event 族 | 生产者 | 消费者 |
|---|---|---|
| SSH/local/Telnet terminal output、state、attachment、input focus 变化 | session actor | 仅 terminal renderer |
| SFTP listing/preview/tail/transfer 与 Forward lifecycle | 独立 SFTP/Forward service | 应用 renderer |
| Metrics sample、stale/error/backoff 与 Overview 聚合 | 独立 Metrics scheduler | Overview renderer |
| Plugin package/readiness/operation/audit/contribution 与 target-context 更新 | plugin lifecycle 与 host bridge | plugins 和已注册 extension surface |
| Plugin API resource event | Plugin API resource registry | 所有者的隔离 Plugin Host，经协议 reply/event 处理 |
| Vault/protected prompt 与 exit blocker 变化 | Core secure state | 被允许的 secure surface/renderer |

内部 emitter 名和传输形态属于实现细节。插件只收到[插件 API](../plugin-api/broker.zh-CN.md)定义的类型化 subscription/resource event，永远不能监听 renderer event channel。Tauri listener、包 event 和不透明 resource handle 都不会传递 authority。

原生托盘事件：`native-tray-action { token }`、`native-tray-error { messageKey }`、`native-tray-vault-changed`。资源通知点击 `native-resource-notification-click` 只携带事件/资源 ID、generation 与 revision，不含命令、文件名、路径或秘密；主窗口重新读取真实快照，过期点击不重连。它们不是插件事件订阅权限。
