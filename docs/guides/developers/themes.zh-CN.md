# 声明式主题插件

主题是宿主解析的纯数据插件。它通过现有本地 ZIP 导入、安装、更新、禁用和卸载流程管理，不启动 Plugin Host，不请求能力，也不能加载 CSS、JavaScript、HTML、字体文件或远程资源。安装不自动替用户选择主题；用户在“设置 → 外观”选择并保存。

## 包契约

包仅包含 `manifest.json` 和 `assets/theme.json`（允许对应目录项）。manifest 保持现有字段及签名格式；主题使用协议 1.14，`capabilities` 为空，不包含 `plugin.wasm`。旧 Wasm 包继续使用协议 1.13 稳定基线，不能把主题资产放进 Wasm 包。混合包、旧协议主题、额外文件及非空能力声明全部拒绝。

本地 ZIP 的发布者信息为自报，不因内容是主题而显示为已验证。主题资产属于精确包哈希的一部分；安装后读取时还要验证实际资产内容摘要，损坏时不提供该主题，也不能转为 Wasm 启动。

三个可重建示例在 `examples/theme-plugins/`：青苔（浅色）、绛夜（深色）、暖砂（浅色）。查看该目录的构建脚本与 README 生成带版本号的 ZIP。新宿主兼容旧 Wasm；旧宿主不会支持新主题包，具体拒绝文案取决于旧版本。

## theme.json

| 字段 | 约束 |
| --- | --- |
| `schemaVersion` | `1` |
| `id` | 1–64 字符的小写主题标识 |
| `name` | `zhCN`、`en` 两个有界名称，以纯文本显示 |
| `appearance` | `light` 或 `dark`；一个包提供一个方案 |
| `colors` | 下列完整语义颜色集合，仅 `#RRGGBB` |
| `fontFamily` | `system`、`sans`、`mono`，由宿主映射字体栈 |
| `fontSize` | 12–18 整数字号 |
| `radius` | 0–12 整数圆角 |
| `borderWidth` | 1 或 2 |
| `density` | `compact`、`standard`、`comfortable` |
| `shadow` | `none`、`soft`、`standard` |
| `terminalPalette` | 可选，完整沿用宿主 TerminalPalette；不改变用户固定终端方案 |

颜色角色：`bgCanvas`、`bgSurface`、`bgSubtle`、`bgHover`、`border`、`borderStrong`、`textPrimary`、`textSecondary`、`textTertiary`、`accent`、`accentHover`、`onAccent`、`accentSoft`、`success`、`successSoft`、`warning`、`warningSoft`、`danger`、`onDanger`、`dangerSoft`、`focusRing`、`terminalPaneActiveBorder`、`brandMarkPrimary`、`brandMarkSecondary`、`selection`、`selectionText`。

JSON 上限 32 KiB，拒绝未知字段与任意 CSS 字符串。宿主检查主要/次要文字、按钮文字、选区、焦点与状态背景的对比度；以实现中的共享契约及示例测试为准。浅深配色须分别设计，不能机械反相。所有令牌只能映射宿主明确允许的 CSS 变量，不开放层级、窗口控制尺寸、内容隐藏或行为修改。

## 选择、自定义和恢复

“外观模式”独立保存浅色、深色或跟随系统偏好；“外观”页分别选择浅/深方案。自定义以 `pluginId + themeId` 绑定的覆盖值保存，不修改包内容。编辑和导入先进入草稿，保存成功后才更新活动投影；取消不影响原设置。完整偏好迁移保留旧文件兼容，单独主题配置导出不携带插件代码或外部主题定义。

禁用、卸载、资产损坏或主题不可用时使用同模式内置主题，保留用户自定义以便恢复。更新后合并覆盖值重新校验，不能把旧覆盖值强制套到不兼容的新配色。跟随应用的终端可采用包内调色板，固定或自定义终端配色仍独立。换色只重绘，不改变 SSH、PTY、输入所有权或插件进程。

普通共享 UI 随宿主主题更新。隔离自绘网页不承诺自动跟随；插件作者仍需接入宿主提供的外观上下文。系统文件选择器与原生窗口控件受操作系统限制。独立安全窗口和页内受保护区域保持宿主内置配色/排版；普通托盘只消费经过校验的已解析外观投影，不加载插件。

## 验证边界

必须验证旧 Wasm 安装/启用、有效与恶意主题包、更新失败回滚、禁用/卸载回退、零主题进程、重启恢复、草稿取消/持久化失败、系统切换、终端固定方案独立，以及真实窗口中的安全区域与可读性。当前完成度、制品及平台证据见[实施状态](../../../README.md#安装与快速开始)，不能从示例存在推断已经发布或跨平台验收完成。

主题保护使用独立 `data-theme-protected` 边界；既有 `data-plugin-protected` 仍表示插件 DOM 访问隔离，不能把普通终端与插件面板都当作安全外观区域。安全模式启动将安装主题标记为不可启用并使用内置外观。
