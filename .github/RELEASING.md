# NoriShell 发布规范

每次发布以同一 Git revision、实际安装包和用户可见变化为准。构建命令与本机环境步骤遵循 `main` 工作树中的 `AGENTS.md` 和 `.build`；创建发布工作树之前先完整读取这两个本地文件，创建后复制到新工作树并核对。本文只规定仓库里需要同步的公开内容和 Release 交付格式。

## 发布前

1. 将应用版本统一到 `package.json`、Cargo workspace/lock 和 `src-tauri/tauri.conf.json`，并确认 Git tag 为 `v<app-version>`。自建同步服务端与插件共用独立的 `<sync-version>`，不要求等于应用版本。
2. 更新英文 `README.md` 和中文 `README.zh-CN.md`：安装区的当前版本、四平台各自的安装包及 ZIP 直达链接必须指向本次 Release 的实际文件；功能、更新方式和限制只描述该版本真实提供的行为。两份 README 各自保持单语。
3. 从 [Release 文案模板](RELEASE_TEMPLATE.md)填写正文。先写本版面向用户的变化，再给下载入口；中文与英文对应，`新增 / 调整 / 修复` 中没有内容的栏目直接删除。描述具体场景和结果，不写任务进度、实现账本、内部验收记录或空泛宣传；影响使用的已知限制应如实说明。

## 交付内容

- macOS ARM64、macOS x64、Windows x64、Windows ARM64：每个平台均提供安装包与完整应用 ZIP，共 8 个应用包。文件名使用 `NoriShell_<app-version>_<platform>_<arch>` 约定，具体名称以模板中的下载链接为准。
- 自建同步服务端与可直接导入的同步插件各 1 个 ZIP；二者使用同一 `<sync-version>`。至此共 10 个用户下载包。
- 自动更新另附 2 个 macOS 更新归档、4 个签名文件及 `latest.json`；`SHA256SUMS.txt` 覆盖以上 17 个资产。Release 总计 18 个附件。

## 发布核对

草稿 Release 的 tag、包内版本、架构、签名、文件名和下载链接必须一致。上传后从 GitHub 下载到独立目录核对资产清单与 SHA-256，再发布为 Latest；随后检查两份 README 的 8 个直达链接及 Release 正文链接都能访问。发布文案只陈述实际交付的内容，不把构建通过写成平台运行验收。
