# 声明式 UI、字段与目标

公开 document schema 是版本化且由宿主渲染的。可按允许使用 `stack`、`grid`、`section`、`divider`、`dialog`；展示节点包括 text/code/status/progress；有界控件包括 button/copy button/text field/select/checkbox/switch/table/menu/disclosure。Core 会拒绝未知 node、跨 document field、未批准 target 和陈旧 revision。

target 只能来自 `ExtensionTargetRegistry`。每个 target 有稳定 id、surface kind、允许 node 集、类型化 contextual projection、尺寸预算与风险级别。target 位置不授予终端、Host、文件系统或网络访问。`ui.page` 和受控 plugin navigation 仍在插件区域内，插件不能插入一级导航、任意 header 控件、宿主 route 或 DOM。

Page password field 只能为同一 action 的受保护 credential 流程提供输入：不得预填、回显、传入 Wasm、持久 state 或 storage。其他 input 都是普通有界值。使用 Core 提供的应用 locale，插件文字不能改变应用语言。声明的 `onOpenActionId` 仍是后台 action：可用非交互数据刷新 document，但不能打开 protected prompt、创建/解锁 Vault、取得 credential 或请求 terminal input。

Page 的 `onOpenActionId` 是独立、无字段的生命周期声明，不能与任何可点击 action（包括禁用按钮、表格行和递归树节点）重名。Core 在初始化及替换文档时复核；执行时从已接纳的 Page 声明判定后台权限，前端标志不能将其升级为明确用户操作。关闭目标 context 后进行中的同步失去继续操作权限。
