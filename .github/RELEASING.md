# NoriShell 发布规范

每次发布以同一 Git revision、实际安装包和用户可见变化为准。构建命令与本机环境步骤遵循 `main` 工作树中的 `AGENTS.md` 和 `.build`；创建发布工作树之前先完整读取这两个本地文件，创建后复制到新工作树并核对。本文只规定仓库里需要同步的公开内容和 Release 交付格式。

## 发布前

1. 将应用版本统一到 `package.json`、Cargo workspace/lock 和 `src-tauri/tauri.conf.json`，并确认 Git tag 为 `v<app-version>`。自建同步服务端与插件共用独立的 `<sync-version>`，不要求等于应用版本。
2. 更新英文 `README.md` 和中文 `README.zh-CN.md`：安装区的当前版本、macOS / Windows 四个目标各自的安装包及 ZIP 直达链接，以及 Linux（预览版）两个架构的 AppImage / deb 直达链接，必须指向本次 Release 的实际文件；功能、更新方式和限制只描述该版本真实提供的行为。两份 README 各自保持单语。
3. 从 [Release 文案模板](RELEASE_TEMPLATE.md)填写正文。先写本版面向用户的变化，再给下载入口；中文与英文对应，`新增 / 调整 / 修复` 中没有内容的栏目直接删除。描述具体场景和结果，不写任务进度、实现账本、内部验收记录或空泛宣传；影响使用的已知限制应如实说明。

## 交付内容

- macOS ARM64、macOS x64、Windows x64、Windows ARM64：每个平台均提供安装包与完整应用 ZIP，共 8 个应用包。文件名使用 `NoriShell_<app-version>_<platform>_<arch>` 约定，具体名称以模板中的下载链接为准。
- Linux x64、Linux arm64（预览版）：每个架构提供 AppImage 与 deb，共 4 个包，文件名为 `NoriShell_<app-version>_linux_<arch>.AppImage` / `.deb`。仅 AppImage 参与应用内更新；deb 只供手动下载。Linux 目前只在 Ubuntu 22.04 GNOME Wayland 上做过验证，须在 Release 正文标注“预览版 / Preview”，并如实说明托盘、原生通知和系统钥匙串自动解锁尚未实现。
- 自建同步服务端与可直接导入的同步插件各 1 个 ZIP；二者使用同一 `<sync-version>`。至此共 14 个用户下载包（8 个 macOS / Windows 应用包 + 4 个 Linux 包 + 2 个同步 ZIP）。
- 自动更新另附 2 个 macOS `.app.tar.gz` 更新归档及 `latest.json`，共 3 个更新专用资产。不再上传单独的 `.sig` 文件：所有平台的签名只保存在 `latest.json` 中，客户端只读取它；`tools/package_updater_release.py` 也不会向暂存目录写入签名，暂存目录中出现 `.sig` 会直接报错。
- `SHA256SUMS.txt` 覆盖以上 17 个资产（14 个下载包 + 2 个更新归档 + `latest.json`），Release 总计 18 个附件。
- Linux 构建元数据（`linux-build-<arch>.json`）不是 Release 资产，只由封包脚本通过 `--linux-metadata-dir` 读取，用于核对 AppImage / deb 的 SHA-256、版本、架构和干净工作区。

## 发布核对

草稿 Release 的 tag、包内版本、架构、签名、文件名和下载链接必须一致。上传后从 GitHub 下载到独立目录核对资产清单与 SHA-256，再发布为 Latest；随后检查两份 README 的 12 个直达链接（8 个 macOS / Windows + 4 个 Linux 预览版）及 Release 正文链接都能访问。发布文案只陈述实际交付的内容，不把构建通过写成平台运行验收。
