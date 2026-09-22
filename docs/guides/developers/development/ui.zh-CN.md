# 声明式 UI document

NoriShell 插件描述的是一套由宿主渲染的小型界面。插件返回 `ui.document` runtime output；不会创建 DOM 节点、注入 CSS、调用 Tauri IPC，也不能控制宿主窗口。Core 会先校验 schema、target、已批准 capability、action、字段和当前 target 上下文，再决定是否渲染或分派。

在 `initialize` 中返回初始 document。用户操作后，返回替换 document 或一条类型化 broker 请求。异步结果到达 `brokerResult` 时，更新实例状态，再为同一 target 返回一份新的**完整** document。回调协议的完整说明见[调用 API 与处理回调](./calling-api.zh-CN.md)。

## document 结构

`targetId` 选择宿主拥有的挂载点；`document` 是完整快照，不是 patch。下文提供完整、可解析的示例。

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `targetId` | 是 | 下表中的一个已注册扩展 target。插件不能自行发明 target。 |
| `document.schemaVersion` | 是 | 当前声明式 UI schema 使用 `1`。 |
| `document.rootNodeId` | 是 | 恰好一个根节点的 `nodeId`。 |
| `document.nodes` | 是 | 扁平的节点数组。具有子节点的节点以 `nodeId` 引用其他项。 |

每个节点对象都有 `kind` 和 `nodeId`。ID 是插件本地标识，不是 DOM selector 或宿主资源 ID。请用稳定且有界的 ID，例如 `summary`、`refreshButton`、`sync:refresh`；不要写空白、路径、markup、密码、token、Host ID 或 handle。未知字段会被拒绝。

`children` 是 node ID 数组，不能嵌入 node JSON。只有下表标出的节点可使用它；`tabs` 的子节点放在每个 tab 对象中。document 中的引用必须有效，不能依赖“省略节点后保留旧界面”的局部更新。不要借 document 模拟任意 HTML 布局、自定义 CSS、JavaScript、IPC、浏览器导航或宿主数据访问：这些都不是声明式 UI 功能。

## 24 种节点

必须使用精确的小驼峰 `kind` 值。字段后的 `?` 表示可选；表中其余字段以及 `kind`、`nodeId` 都必填。

| `kind` | 除 `kind`、`nodeId` 外的字段 | 用途 |
| --- | --- | --- |
| `stack` | `direction`, `align`, `gap`, `children` | 水平或垂直排列子节点。 |
| `grid` | `columns`, `columnWeights?`, `gap`, `children` | 使用宿主管理的网格排列子节点；若有 `columnWeights`，每列都有一个正值。 |
| `section` | `title?`, `children` | 将相关内容分组，可带标题。 |
| `divider` | — | 分隔相邻的宿主渲染内容。 |
| `text` | `text`, `style`, `tone` | 以宿主文字样式和语气展示文本。 |
| `code` | `text`, `language?`, `wrap` | 展示有界代码或诊断文本；它不是可编辑终端。 |
| `icon` | `icon`, `accessibleLabel`, `tone` | 展示带无障碍名称的宿主图标。 |
| `status` | `label`, `tone` | 展示简短状态标签。 |
| `progress` | `label?`, `valuePercent?`, `tone` | 展示宿主进度；未知进度时省略 `valuePercent`。 |
| `button` | `actionId`, `label`, `icon?`, `variant`, `disabled` | 发起一项明确的插件 UI action。 |
| `copyButton` | `actionId`, `label`, `disabled` | 明确操作后，请求宿主单独校验的剪贴板写入。 |
| `textField` | `fieldId`, `label`, `value`, `placeholder?`, `fieldKind`, `required`, `disabled` | 收集文本、搜索、数字、URL、多行文本，或仅 page 可用的插件自有密码。 |
| `select` | `fieldId`, `label`, `value?`, `options`, `disabled` | 在声明的选项中选择一个值；每项有 `value`、`label`、`disabled`。 |
| `checkbox` | `fieldId`, `label`, `checked`, `disabled` | 收集布尔选项。 |
| `switch` | `fieldId`, `label`, `checked`, `disabled` | 用 switch 形式收集布尔设置。 |
| `table` | `label`, `columns`, `rows`, `emptyText?` | 渲染有界表格。column 有 `columnId`、`label`、`width?`；row 有 `rowId`、`cells`、`actionId?`。 |
| `menu` | `label`, `children` | 在宿主菜单中分组子 action。 |
| `dialog` | `title`, `description?`, `triggerLabel`, `closeLabel`, `children` | 宿主拥有、由用户触发的 dialog；打开状态、关闭和焦点圈定由 renderer 管理。 |
| `disclosure` | `label`, `open`, `children` | 展示或折叠宿主 disclosure 区块。 |
| `sshSyncBrowser` | `profileId`, `children` | 仅 page 可用的宿主 SSH 同步数据浏览器；插件不会取得其搜索、选择或分页状态。 |
| `tabs` | `label`, `tabs` | 展示宿主 tab；每个 tab 有 `id`、`label`、`children`。 |
| `tree` | `label`, `items` | 渲染树；每个 item 有 `id`、`label`、`children?`、`actionId?`。 |
| `chart` | `label`, `chartKind`, `series`, `labels` | 渲染宿主 line 或 bar 数据；每个 series 有 `label` 和有限数值 `values`。 |
| `editor` | `fieldId`, `label`, `value`, `language`, `readOnly` | 渲染宿主编辑器字段。 |

固定 enum 值如下：`direction` 为 `horizontal` 或 `vertical`；`align` 为 `start`、`center`、`end`、`stretch`；文本 `style` 为 `body`、`secondary`、`caption`、`heading`、`monospace`；`tone` 为 `neutral`、`info`、`success`、`warning`、`danger`；按钮 `variant` 为 `primary`、`secondary`、`ghost`、`danger`；`fieldKind` 为 `text`、`search`、`number`、`url`、`multiline`、`password`；`chartKind` 为 `line` 或 `bar`。

### 字段、表单与受保护界面

只有 `textField`、`select`、`checkbox`、`switch`、`editor` 会生成字段值。Core 只会在 action 已验证后发送当前字段，并按声明该 action 的 document 过滤字段。字段是未可信的用户输入，仍要为实际操作校验；不能用它模拟用户身份，或让它选择不受限的宿主资源。

表单节点只能放在下表 `Forms` 为“是”的 target。`textField.fieldKind: "password"` 仅允许出现在插件自有的 `app.page`，用于插件自身的凭据流程；它不能读取 Vault、SSH 凭据、其他应用或宿主密码。`sshSyncBrowser` 也仅允许在 `app.page`；远端数据交互仍由 Core 拥有。`dialog` 可贡献到 `app.header.actions` 或 `app.page`，但不会让插件控制窗口、焦点或关闭。

## 已注册的挂载 target

下表是完整 target 列表。`Context` 为“是”表示 renderer 会先打开一个具体、短生命周期的 target 实例，再向插件请求 contribution。`Forms` 为“是”表示常规表单节点能在该处通过校验。“否”是边界，不能用 text 或自定义 markup 伪造表单。

| Target | Surface | 所需 capability | Context | Forms |
| --- | --- | --- | --- | --- |
| `plugins.page` | inline | `uiPanel` | 否 | 是 |
| `app.header.actions` | toolbar | `uiPanel` | 否 | 否 |
| `app.content.before` | inline | `uiPanel` | 是 | 是 |
| `app.content.after` | inline | `uiPanel` | 是 | 是 |
| `app.content.sidebar` | sidebar | `uiPanel` | 是 | 是 |
| `app.content.footer` | inline | `uiPanel` | 是 | 是 |
| `app.content.floating` | overlay | `uiPanel` | 是 | 是 |
| `terminal.tools` | sidebar | `uiPanel` | 是 | 是 |
| `terminal.header` | inline | `uiPanel` | 是 | 是 |
| `terminal.footer` | inline | `uiPanel` | 是 | 是 |
| `terminal.floating` | overlay | `uiPanel` | 是 | 是 |
| `terminal.toolbar` | toolbar | `uiPanel` | 是 | 是 |
| `terminal.sidebar` | sidebar | `uiPanel` | 是 | 是 |
| `terminal.contextMenu` | menu | `uiPanel` | 是 | 否 |
| `terminal.annotation` | overlay | `terminalAnnotation` | 是 | 否 |
| `sftp.toolbar` | toolbar | `uiPanel` | 是 | 是 |
| `sftp.contextMenu` | menu | `uiPanel` | 是 | 否 |
| `sftp.transfer.actions` | inline | `uiPanel` | 是 | 否 |
| `hosts.toolbar` | toolbar | `uiPanel` | 否 | 是 |
| `host.detail.tools` | inline | `uiPanel` | 是 | 是 |
| `overview.toolbar` | toolbar | `uiPanel` | 否 | 是 |
| `overview.card.actions` | card | `uiPanel` | 是 | 否 |
| `tunnels.toolbar` | toolbar | `uiPanel` | 否 | 是 |
| `settings.tools` | inline | `uiPanel` | 否 | 是 |
| `commandPalette` | menu | `uiPanel` | 否 | 是 |
| `app.navigation` | navigation | `uiNavigation` | 否 | 否 |
| `app.page` | page | `uiPage` | 是 | 是 |

`routePaths?` 属于普通应用内容的 UI template，表示允许挂载的精确应用路径；只在要适用于每个获准普通挂载点时省略它。`onOpenActionId?` 是无字段的可见性 hook，和用户 action 分开。`autoRefresh?` 只允许宿主为 `terminal.footer` 定时调度，它含有 `actionId` 和 `intervalMs`，不能替代用户批准。

## target context 的边界

Core 为 contextual target 提供 `targetId`、`surfaceKind`、`contextHandle`、`targetRevision`、`displayLabel?`。公开 UI action 请求把不透明句柄称为 `contextHandle`；有些宿主集成将相同概念写作 `targetContextHandle`。无论名称如何，它都不是插件数据。

不要创建、解析、持久化、猜测、共享或复用 context handle。它是短生命周期对象，并受 owner、插件 generation、target 和 target revision 共同约束。每次 UI action，Core 都会检查当前 handle 与 revision；过期 target 是需要刷新的冲突，不能借机重放 action。contextual 插件的临时状态也应限定在给定 target，在 target 关闭时丢弃。对于 `app.page`，Core 还会将活动 context 绑定到发起请求的插件和 page identity。

插件只能得到 Core 为该 action 明确授权的 host metadata 或 DOM snapshot。target context 不授予广泛 DOM 查看、CSS 注入、IPC、宿主导航、终端访问、凭据访问或跨 target 访问能力。

## 最小完整 dialog document

下面是可解析的 `ui.document` payload，包含 `dialog`、`button`、`code`。它使用允许 dialog 的 `app.header.actions`，但没有表单字段。所有 child node 都在同一 document 中。

```json
{
  "targetId": "app.header.actions",
  "document": {
    "schemaVersion": 1,
    "rootNodeId": "apiDemoDialog",
    "nodes": [
      {
        "kind": "dialog",
        "nodeId": "apiDemoDialog",
        "title": "NoriShell API demo",
        "description": "Ask Core for the typed plugin API description.",
        "triggerLabel": "API demo",
        "closeLabel": "Close API demo",
        "children": ["intro", "describe", "details"]
      },
      {
        "kind": "text",
        "nodeId": "intro",
        "text": "No host, terminal, file, network, or Vault access is implied.",
        "style": "caption",
        "tone": "neutral"
      },
      {
        "kind": "button",
        "nodeId": "describe",
        "actionId": "api-demo:describe",
        "label": "Query API",
        "icon": "info",
        "variant": "primary",
        "disabled": false
      },
      {
        "kind": "code",
        "nodeId": "details",
        "text": "Select Query API to ask Core for its typed API description.",
        "language": "text",
        "wrap": true
      }
    ]
  }
}
```

用 SDK helper 生成它，避免手工拼接带转义的 payload JSON：

```rust
use norishell_plugin_sdk::{output, PluginError, PluginRuntimeOutput};
use serde_json::json;

fn document_output(request_id: &str, details: &str)
    -> Result<PluginRuntimeOutput, PluginError>
{
    output(request_id, "ui.document", &json!({
        "targetId": "app.header.actions",
        "document": {
            "schemaVersion": 1,
            "rootNodeId": "apiDemoDialog",
            "nodes": [/* complete node array, including `details` */]
        }
    }))
}
```

这里的 `/* ... */` 是 `json!` 中的 Rust 源码，不是 JSON。真实 output 必须使用上面完整的节点数组。可以继续参考可运行的 [API Demo](../examples/api-demo.zh-CN.md)。

## action、broker 请求与完整替换

常见的异步顺序如下：

```text
initialize -> ui.document（target A，初始完整 document）
用户点击 -> uiAction（actionId，当前已验证字段）
插件 -> api.request（callId，类型化 operation）
Core -> brokerResult（result.kind = "api"，包含对应 callId 的 reply）
插件 -> ui.document（target A，完整替换 document）
```

普通的用户 `UiAction` 必须为同一 target 返回一份完整 document，或启动一项明确的异步 broker operation。不要返回 document fragment，不要把切换 target 当作副作用，也不要在 `brokerResult` 之前显示操作成功。broker 请求等待期间，用当前消息的 `requestId` 调用 `api_request`；回调到来时，用这条回调新的 `requestId` 输出替换 document。

```rust
use norishell_plugin_sdk::{
    api_request, payload, PluginApiOperation, PluginApiOutcome,
    PluginApiReply, PluginError, PluginHostMessageKind, PluginHostRequest,
    PluginRuntimeOutput,
};
use serde_json::Value;

const DESCRIBE_ACTION: &str = "api-demo:describe";

fn handle_ui(request: &PluginHostRequest) -> Result<Vec<PluginRuntimeOutput>, PluginError> {
    let body: Value = payload(request)?;
    match request.kind {
        PluginHostMessageKind::UiAction
            if body["actionId"].as_str() == Some(DESCRIBE_ACTION) =>
        {
            Ok(vec![api_request(
                &request.request_id,
                "api-demo.describe",
                PluginApiOperation::Describe {},
            )?])
        }
        PluginHostMessageKind::BrokerResult => {
            let result = &body["result"];
            if result["kind"].as_str() != Some("api") {
                return Err(PluginError::InvalidRequest);
            }
            let reply: PluginApiReply = serde_json::from_value(result["reply"].clone())
                .map_err(|_| PluginError::InvalidRequest)?;
            if reply.call_id != "api-demo.describe" {
                return Err(PluginError::InvalidRequest);
            }
            let details = match reply.outcome {
                PluginApiOutcome::Completed { value } => format!("{value:#?}"),
                PluginApiOutcome::Failed { code } => format!("Core rejected describe: {code:?}"),
            };
            // 先更新本地非秘密状态，再完整替换 target A。
            Ok(vec![document_output(&request.request_id, &details)?])
        }
        _ => Err(PluginError::InvalidRequest),
    }
}
```

`callId` 只关联 operation，既不是 capability grant，也不是 target handle。每个未完成 operation 使用不同的合法 call ID，并在修改对应 UI 状态前匹配结果。操作可能失败、被拒绝、被撤销、发生冲突，或以未知结果结束；应显示稳定错误，并等待适当的明确重试。operation 与 handle 生命周期见[资源、权限与状态管理](./resources.zh-CN.md)；需要已验证 action 的 API 方法可参考 [appRegister](../../plugin-api/appRegister.zh-CN.md)。

## 实用检查表

1. 声明 target 所需 capability，并等待用户批准。
2. 在 `initialize` 返回一份有效的完整 document。
3. 只使用上述 24 个节点和已注册 target ID；不要添加 HTML、CSS、DOM、IPC 或虚构字段。
4. 字段节点放在接受表单的 target；password 与 SSH 同步浏览器只放在 `app.page`。
5. 将 `contextHandle` / `targetContextHandle`、revision、字段和宿主提供的 metadata 视为有范围的输入，而不是可复用的授权。
6. 异步操作要等待匹配的 `brokerResult`，再完整替换同一 target 的 document。
