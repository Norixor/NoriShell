<p align="center">
  <img src="src/assets/branding/norishell-app-icon.png" width="96" alt="NoriShell" />
</p>
<h1 align="center">NoriShell</h1>
<p align="center">本地优先的终端与远程连接工作台</p>
<p align="center">SSH · 本地终端 · SFTP · RDP / VNC · 加密 Vault · 插件</p>
<p align="center"><a href="LICENSE">GPL-3.0-only</a> · macOS / Windows · 开发中</p>

NoriShell 将远程连接、终端会话、文件传输和凭据管理放进一个桌面应用。基础连接无需登录账号，主机配置保存在本机，已保存的密码与私钥进入加密 Vault；需要跨设备使用时，可通过 Norixor 插件启用同步。

基于 **Tauri 2、Vue 3、TypeScript、xterm 和 Rust** 构建。

## 能做什么

| 能力 | 说明 |
| --- | --- |
| SSH 与本地终端 | 多标签、分屏、最近连接，以及独立的本地 Shell 会话 |
| 主机与连接管理 | 主机分组、密码与密钥认证、跳板机、HTTP CONNECT / SOCKS5 连接入口 |
| 文件传输与隧道 | SFTP 文件浏览与传输、SSH 端口转发 |
| 远程桌面 | RDP / VNC 配置与连接，以及 SSH 网关 |
| 凭据保护 | 加密 Vault、服务器指纹核验、受保护的凭据交互 |
| 服务器概览 | 按主机查看活动资源，以及启用监控后的系统指标 |
| 日常操作 | 自定义快捷键、快捷命令、主题、托盘面板与通知偏好 |
| 插件与同步 | 隔离的 WebAssembly 插件、细粒度权限，以及可选的 Norixor 加密同步 |

> 当前处于开发阶段。功能实现与平台验收仍在推进，macOS 与 Windows 的验证覆盖并不完全相同。远程桌面及跨设备同步仍需进一步真实环境验收；源码可用不代表已提供正式签名、完整跨平台验收的稳定安装包。

## 本地优先，按需连接

- **无需账号即可使用基础功能。** 云服务与同步属于可选能力。
- **秘密交给 Vault。** 持久化密码与私钥加密保存，不作为普通配置明文存储。
- **连接前核验服务器身份。** 首次 SSH 指纹需要确认，已信任指纹发生变化时阻断连接。
- **插件按权限运行。** 插件运行在隔离宿主中，不能直接读取 Vault 秘密或访问主应用内部对象。
- **同步由 Core 处理。** Norixor 插件提供操作入口；加密、比较与恢复由 NoriShell Core 管理。

这些是设计与实现边界，不是独立安全审计结论。漏洞报告方式见 [SECURITY.md](SECURITY.md)。

## 从源码运行

### 环境

- Node.js **22 或更新版本**。
- pnpm **10.32.1**，与 `package.json` 一致。
- Rust **1.97.1**，由 `rust-toolchain.toml` 固定。
- [Tauri 平台依赖](https://v2.tauri.app/start/prerequisites/)：macOS 需要 Xcode Command Line Tools；Windows 需要 MSVC C++ 构建工具与 WebView2。

```sh
git clone https://github.com/Norixor/NoriShell.git
cd NoriShell
pnpm install --frozen-lockfile
pnpm tauri dev
```

私有预览阶段，克隆需要拥有仓库访问权限。

### 检查与构建

```sh
# 生成契约一致性、类型、Lint、前端测试与构建
pnpm check

# Rust 格式、测试与静态检查
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings

# 在当前平台构建桌面应用
pnpm tauri build
```

正式分发还需要对应平台的签名、打包与验收。不要将本地生成的未签名构建视为官方发行版。

## 项目结构

```text
src/               Vue 界面、状态投影与共享组件
src-tauri/         Tauri 桌面入口与宿主集成
crates/            Rust Core、连接协议、Vault、持久化与插件能力
examples/plugins/  插件示例
vendor/            保留上游许可的依赖补丁
```

Rust Core 管理连接、凭据、持久化与资源生命周期；前端负责交互和可重建的视图状态。第三方补丁的来源与修改范围见 [vendor/README.md](vendor/README.md)。

## 插件

本仓库包含插件运行框架、SDK 与通用示例；正式官方插件的实现不包含在本仓库中。插件使用受限 WebAssembly ABI，通过宿主提供的能力工作。可从 [应用页面示例](examples/plugins/app-demo/README.md)、[协议示例](examples/plugins/protocol-demo/README.md)和 [SDK 工具](examples/plugins/sdk-tooling/README.md)了解代码结构。

Norixor 同步服务独立运营。客户端源码许可不包含官方服务访问凭据，也不代表服务免费或无限量可用。

## 许可证

NoriShell 原创代码采用 **GNU General Public License v3.0 only（GPL-3.0-only）**。完整条款见 [LICENSE](LICENSE)。你可以使用、修改和分发本软件；分发受 GPL 覆盖的版本时，须遵守相应源码提供及其他许可义务。

第三方源码、依赖和素材保留各自许可，见 [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)。
