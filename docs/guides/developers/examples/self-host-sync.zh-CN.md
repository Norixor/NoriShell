# 自建同步示例

完整源码位于 [`example/self-host-sync`](../../../../example/self-host-sync/readme.md)：`plugin/` 是声明式 Wasm 插件，`server/` 是单用户 Go 服务端，`build-release.py` 生成可附在 NoriShell Release 的伴生 ZIP。新版插件使用 `uiNavigation`、`uiPage`、高风险 `sshSync`，并要求最低 Core API 1.89；登录表单由宿主安全流程读取，插件只声明同源 HTTP/HTTPS endpoint 与同步动作，不接收 Vault 明文、同步密钥、密码、令牌或 exchange body。HTTP 会暴露传输中的密码、令牌与请求元数据。

从仓库根目录执行 `PATH=<go-bin>:$PATH python3 example/self-host-sync/build-release.py`，需 Go 1.23+、项目锁定的 Rust 1.97.1 及 `wasm32-unknown-unknown` target。打包脚本交叉编译六种服务端二进制，运行真实 Wasm Plugin Host 验证器和 SDK 的 `pack`/`check`，再生成服务端与插件两个独立 ZIP。未经本地导入、授权和原生验收，构建成功不等于已安装或已发布。

新插件只选择 hosts、credentials、desktopProfiles，再编排 HTTP GET/PUT、ETag/CAS、对象冲突与同步方向。Core 提供按类别授权的快照、检查、组合、应用、导出与基线提交，独占加密、Vault、本机提交和经过网络收据验证的基线更新。`networkStart` 的 `httpExchange` 把请求和响应密文 Blob 留在 Core，不送入 Wasm。应用偏好与终端历史另有 Core 独立读取接口，但此插件不调用。旧 Core 一键 Refresh/Sync 与应用级调度正在替换；插件自动同步策略和执行仍待完成，不能把旧调度写成新契约。旧 V4/V5 包必须先认证再投影为三类 V6 数据。Go 服务端继续以独立服务密码和强 ETag/CAS 守住存储边界。

Core 对新远端使用由插件 ID 稳定派生的数据 owner；既有远端则先认证原包哈希 owner 的密文，再沿用原 owner 和本地映射。包 SHA-256 仍绑定授权及账号令牌。新设备需要通过明确的同步操作恢复原 Vault 同步密钥，后台页面刷新不会弹出密码窗口。

部署步骤见[用户指南](../../users/self-host-sync.zh-CN.md)，具体 HTTP 契约和单实例限制见 [`server/README.md`](../../../../example/self-host-sync/server/README.md)。
