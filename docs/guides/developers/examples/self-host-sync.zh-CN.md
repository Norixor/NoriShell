# 自建同步示例

完整源码位于 [`example/self-host-sync`](../../../../example/self-host-sync/readme.md)：`plugin/` 是声明式 Wasm 插件，`server/` 是单用户 Go 服务端，`build-release.py` 生成可附在 NoriShell Release 的伴生 ZIP。插件使用 `uiNavigation`、`uiPage` 和高风险 `sshSync`；登录表单由宿主安全流程读取，插件只声明同源 HTTP/HTTPS endpoint 与同步动作，不接收 Vault 明文、同步密钥、密码、令牌或 exchange body。HTTP 会暴露传输中的密码、令牌与请求元数据。

从仓库根目录执行 `PATH=<go-bin>:$PATH python3 example/self-host-sync/build-release.py`，需 Go 1.23+、项目锁定的 Rust 1.97.1 及 `wasm32-unknown-unknown` target。打包脚本交叉编译六种服务端二进制，运行真实 Wasm Plugin Host 验证器和 SDK 的 `pack`/`check`，再生成服务端与插件两个独立 ZIP。未经本地导入、授权和原生验收，构建成功不等于已安装或已发布。

应用运行期自动同步调度由 Core 持有，不由插件 Wasm 定时调用：每次重新核对安装包、实例、授权、设置 URL、Vault 和同步基线。V5 三方合并按项目更新时间选择较新版本；冲突与删除有独立的用户策略，策略从宿主持久设置读取，Wasm 请求不能扩大自动覆盖权限。缺少可信时间、首次基线或待办时暂停自动写入；启动检查只读。八组非秘密偏好由主窗口的宿主适配器导出和逐组 CAS 恢复；收集快照不能伪造修改时间，缺少真实时间的双端冲突需要人工处理。Core 在 SSH/桌面恢复前持久保存偏好待办，结果不确定时必须由用户明确清除并重新同步。服务端只验证其独立的服务密码、管理会话、执行 ETag/CAS 并原样保存 Core 密文。

Core 对新远端使用由插件 ID 稳定派生的数据 owner；既有远端则先认证原包哈希 owner 的密文，再沿用原 owner 和本地映射。包 SHA-256 仍绑定授权及账号令牌。新设备需要通过明确的同步操作恢复原 Vault 同步密钥，后台页面刷新不会弹出密码窗口。

部署步骤见[用户指南](../../users/self-host-sync.zh-CN.md)，具体 HTTP 契约和单实例限制见 [`server/README.md`](../../../../example/self-host-sync/server/README.md)。
