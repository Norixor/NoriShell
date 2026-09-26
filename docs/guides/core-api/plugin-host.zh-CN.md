# Plugin Host bridge 边界

应用 IPC 负责包生命周期、受保护 Dialog 和宿主渲染投影；隔离 guest 通过 SDK ABI 以 protocol 1.13 与 Plugin Host 通信。Core 注入包身份、不可变 package hash、grant、owner、scope 与 generation，并拒绝 guest 自报的对应字段。

`plugin_*` Core command 只能由拥有这些应用界面的 NoriShell renderer 或 ACL 专门限定的 secure surface 使用。插件作者应使用 SDK `api.request`、不透明 handle 与 Core callback。`api.request` 在真正执行时还会再准入一次，绝不会变成调用 Tauri IPC 的权限。

这一分隔确保 Plugin page、Wasm guest、声明式 UI、isolated WebView 或 host DOM contribution 均不能到达 Vault、SSH internal、SQLite、原始 transport/channel、内部 Core command 或 secure prompt。后台 `onOpen`、timer、restart recovery 与 provider 路径必须返回 `interactionRequired` 或 Vault 状态等类型化非交互结果，不能自行创建、解锁或恢复 Vault 交互。

Core API 1.88 在 `plugin_data_api` 增加按类别划分的 Plugin API DTO，只能通过 Broker 并经过权限与运行检查使用；插件仍不得调用 renderer IPC。原生 exchange 验收尚未完成。
