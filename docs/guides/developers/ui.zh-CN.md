# 声明式 UI 与扩展目标

插件返回版本化的声明式 document，不是 HTML、Vue component、CSS、route、script、任意 DOM event 或 Tauri command。Core 对 node、field、action id、document 大小、深度、列表、patch 和 action 并发都有限制；文本是不可信输入。贡献仅在相同 package、instance、target、context handle、generation 与 document revision 下接受。

只能挂载到 Core 注册的 target。每个 target 自己规定允许 node、布局预算、contextual projection 与风险级别。Page 和 navigation contribution 只能在受控插件分组中出现，不能新建系统 navigation、修改 Global Header 或成为 Terminal workspace 状态。secure route 与 secure surface 都被排除。

使用宿主 locale 与共享控件。password field 仅能出现在插件 Page：初值为空、不会返回 Wasm、提交后清空，而且只能由同一 action 的受保护 credential broker 消费。完整 node、field 和 target 目录见[插件 API 参考](../plugin-api/ui.zh-CN.md)。

普通 `UiAction` 必须返回恰好一份同 target 的有效 `ui.document`；即使动作只是输出 `ui.webview.open` 打开隔离窗口，也要同时保留或更新该 document。只有 `api.request`、资源 broker 和远端操作 broker 可先返回零份 document，随后由 `BrokerResult` 提供替换 document。SDK/Wasm 验证应覆盖完整动作输出及 Core 校验，不能仅验证某个输出 JSON 或 ABI 调用成功。

`clipboard.write` 的声明式复制结果支持最多 16 KiB 的多行文本，允许 LF、CR、Tab，拒绝其他控制字符；只能由已渲染的 `CopyButton` 经明确点击及 Core 权限/上下文栅栏后写入。旧 `ui.panel` 的预声明 copyValues 仍是 512 字节单行值。隔离页面请求 `surfaceId: "toolbox"` 时，ZIP 必须包含 `assets/isolated/toolbox.html`；包装页通过唯一 `{type: "norishell.bridge.ready", protocol: 13, locale, theme}` 消息交付 MessagePort。
