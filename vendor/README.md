# 第三方源码与本地补丁

这些目录保留各自上游许可证，不适用仓库原创代码的GPL-3.0-only 许可证声明。Cargo 的 `[patch.crates-io]` 与相关 crate 的路径依赖指定实际使用的副本；不得直接用 registry 版本替换而丢弃本地改动。

以下清单以相同名称及版本的 crates.io 发布包为比较基线；修改文件带有 NoriShell 标记，原版权声明保留。更新补丁时同步此清单、修改文件标记及相关协议回归测试。需要具体差异时，对照所列发布包与本目录源码；本清单不将上游 revision 冒充本项目提交。

### IronRDP 9b151c4c2e47c6014e1e8e55909d4180aa8bdb99 (当前使用)

- 来源：https://github.com/Devolutions/IronRDP ，固定 commit `9b151c4c2e47c6014e1e8e55909d4180aa8bdb99`；本地快照位于 `vendor/ironrdp-upstream/`，保留上游 MIT 与 Apache-2.0 许可证。根 Cargo `[patch.crates-io]` 与 RDP 客户端直接依赖锁定本地路径，`Cargo.lock` 固定第三方依赖版本。
- 只保留本应用当前 IronRDP 依赖闭包：`ironrdp`、`ironrdp-async`、`ironrdp-bulk`、`ironrdp-cliprdr`、`ironrdp-connector`、`ironrdp-core`、`ironrdp-displaycontrol`、`ironrdp-dvc`、`ironrdp-egfx`、`ironrdp-error`、`ironrdp-graphics`、`ironrdp-input`、`ironrdp-pdu`、`ironrdp-rdpei`、`ironrdp-rdpemt`、`ironrdp-rdpeudp`、`ironrdp-rdpeudp-tokio`、`ironrdp-rdpsnd`、`ironrdp-session`、`ironrdp-svc`、`ironrdp-tls`。上游源码不含本地 Git 元数据，原有版权和许可证保留。
- 上游工作区 `Cargo.toml` 只列入所需 crates；meta crate `crates/ironrdp/Cargo.toml` 去掉未引入的可选 crate、其 feature 与示例/dev 依赖。这是打包路径调整，不改变实际启用的协议能力。
- `ironrdp-connector/src/connection_activation.rs`：迁移 Demand Active 对 Fast-Path、Surface Commands、Bitmap Codecs、Frame Acknowledge 的逐次限制；重激活重新核对，保留有损位图 drawing flags。上游已支持 `connection_type`，本地不重复修改。
- `ironrdp-session/src/fast_path.rs`：上游已经按 bitmap 源 stride 与行 padding 解码并包含对应测试，沿用这一实现；`src/active_stage.rs` 让纯 UDP DVC 的 EGFX EndFrame 同步 drain/composite，向调用方返回 `GraphicsUpdate` 和帧确认；`src/image.rs` 在重设帧缓冲区分配前施加应用相同的单维 8192、总像素 16,777,216 上限。
- `ironrdp-egfx/src/decode.rs`：OpenH264 解出位流尺寸后、分配 RGBA 输出前施加相同的单维和像素上限，并检查 RGBA 字节长度溢出；拒绝过大帧的定向测试不分配大缓冲区。
- `ironrdp-rdpsnd/src/client.rs` 与 `src/client/tests.rs`、该 crate `Cargo.toml`：保留 NoriShell 严格的分包 WaveInfo/Wave 长度与状态校验、错误时仅关闭声音通道、样本与调试信息脱敏；启用 lib 回归测试。
- `ironrdp-tls/src/rustls_verifier.rs`：禁用 `SSLKEYLOGFILE` 流量密钥输出；仅 UnknownIssuer 可交由审批回调，过期等其他证书错误直接拒绝。
- `ironrdp-rdpeudp-tokio/src/transport.rs`：可选的 `expected_leaf_der` 在 UDP TLS 握手后、RDPEMT 协商前精确匹配主 TCP TLS 的叶证书；失败即中断，字段不输出证书内容。调用方必须显式选择严格证书校验；此补丁没有放宽 UDP TLS 默认信任策略。

原有三个 `vendor/ironrdp-connector`、`vendor/ironrdp-session`、`vendor/ironrdp-rdpsnd` 目录保留供旧补丁追溯，当前 Cargo 不再解析它们。下面的旧版本清单只描述其历史基线。

### ironrdp-connector 0.10.0

- 上游：https://github.com/Devolutions/IronRDP
- crates.io 基线：`ironrdp-connector 0.10.0`；上游 revision：`11a0810cfbbabd8b8023875a05e3041216d4b01b`。
- 许可证：`MIT OR Apache-2.0`，正文保留在本目录对应子目录。
- 相对发布包发生修改的文件（忽略 CRLF/LF 差异）：`Cargo.toml`、`src/connection_activation.rs`。

### ironrdp-rdpsnd 0.9.0

- 上游：https://github.com/Devolutions/IronRDP
- crates.io 基线：`ironrdp-rdpsnd 0.9.0`；上游 revision：`11a0810cfbbabd8b8023875a05e3041216d4b01b`。
- 许可证：`MIT OR Apache-2.0`，正文保留在本目录对应子目录。
- 相对发布包发生修改的文件（忽略 CRLF/LF 差异）：`Cargo.toml`、`README.md`、`src/client.rs`。

### ironrdp-session 0.11.0

- 上游：https://github.com/Devolutions/IronRDP
- crates.io 基线：`ironrdp-session 0.11.0`；上游 revision：`11a0810cfbbabd8b8023875a05e3041216d4b01b`。
- 许可证：`MIT OR Apache-2.0`，正文保留在本目录对应子目录。
- 相对发布包发生修改的文件（忽略 CRLF/LF 差异）：`Cargo.toml`、`src/fast_path.rs`。

### picky 7.0.0-rc.25

- 上游：https://github.com/Devolutions/picky-rs
- crates.io 基线：`picky 7.0.0-rc.25`；上游 revision：`18a3a419adcbe0034f7fdfc694039cc970d53766`。
- 许可证：`MIT OR Apache-2.0`，正文保留在本目录对应子目录。
- 相对发布包发生修改的文件（忽略 CRLF/LF 差异）：`Cargo.toml`、`Cargo.toml.orig`。

### russh 0.63.1

- 上游：https://github.com/warp-tech/russh
- crates.io 基线：`russh 0.63.1`；上游 revision：`d3ae702a43a163946f258297e398dc216339d5ce`。
- 许可证：`Apache-2.0`，正文保留在本目录对应子目录。
- 相对发布包发生修改的文件（忽略 CRLF/LF 差异）：`Cargo.toml`、`Cargo.toml.orig`、`src/auth.rs`、`src/channels/mod.rs`、`src/client/encrypted.rs`、`src/client/kex.rs`、`src/client/mod.rs`、`src/kex/curve25519.rs`、`src/kex/dh/mod.rs`、`src/kex/ecdh_nistp.rs`、`src/kex/hybrid_mlkem.rs`、`src/kex/mod.rs`、`src/kex/none.rs`、`src/keys/agent/client.rs`、`src/keys/format/mod.rs`、`src/keys/format/pkcs5.rs`、`src/keys/format/pkcs8_legacy.rs`、`src/negotiation.rs`、`src/server/kex.rs`、`src/session.rs`。

### russh-sftp 2.4.0

- 上游：https://github.com/AspectUnk/russh-sftp
- crates.io 基线：`russh-sftp 2.4.0`；上游 revision：`e145c1f7ece99f41f558949ef59731f2cd1a9dfe`。
- 许可证：`Apache-2.0`，正文保留在本目录对应子目录。
- 相对发布包发生修改的文件（忽略 CRLF/LF 差异）：`src/client/fs/file.rs`、`src/client/mod.rs`、`src/client/rawsession.rs`、`src/client/session.rs`、`src/extensions.rs`。

### sspi 0.21.3

- 上游：https://github.com/devolutions/sspi-rs
- crates.io 基线：`sspi 0.21.3`；上游 revision：`09088ac49cf13449656dca94b68f5228919a4d95`。
- 许可证：`MIT OR Apache-2.0`，正文保留在本目录对应子目录。
- 相对发布包发生修改的文件（忽略 CRLF/LF 差异）：`Cargo.toml`、`Cargo.toml.orig`。

### tauri 2.11.5（`vendor/tauri-reparent/`）

- 上游：https://github.com/tauri-apps/tauri
- crates.io 基线：`tauri 2.11.5`；根 Cargo `[patch.crates-io]` 指向本目录。用途：Workspace Tab 子 WebView 在窗口间 reparent（`src-tauri/src/workspace_tab_views.rs`）所需的最小补丁。
- 许可证：`Apache-2.0 OR MIT`，正文保留在本目录 `LICENSE_APACHE-2.0`、`LICENSE_MIT`。
- 相对发布包发生修改的文件（未计入被忽略的 `mobile/android/build` 生成目录）：
  - `src/manager/mod.rs`：`emit` / `emit_filter` 先克隆 WebView 列表并释放 webviews 注册表锁，再向 JS 派发；上游在派发期间持锁，与等待事件循环的原生 reparent 互相等待时会死锁。
  - `src/webview/mod.rs`：`Webview::reparent` 在 runtime 成功后才更新 Rust 侧父窗口；上游先写入，失败时留下错误父窗口。
  - `src/window/mod.rs`：`is_webview_window` 由 `all` 改为 `any`，同名主 WebView 存在即视为 WebviewWindow。上游在添加子 WebView 后会把 `main` 等窗口判定为非 WebviewWindow，导致 `get_webview_window("main")` 与 `WebviewWindow` 命令参数失效。
  - `src/webview/webview_window.rs`、`src/lib.rs`：`WebviewWindow` 命令参数、`get_webview_window` 与 `webview_windows` 额外要求 WebView label 等于窗口 label，子 WebView 不会被配成其父窗口的 WebviewWindow（保持上游对子 WebView 返回 `None` / 拒绝的语义）。
  - `src/menu/plugin.rs`：`popup` 命令只持有克隆出的菜单 `Arc`，不在原生弹出（macOS 为嵌套模态循环）期间持有 resources table 锁，避免期间需要该表的 IPC 死锁。
- 语义边界：`is_webview_window` 其余内部调用点为 `manager/window.rs` 的窗口级拖放事件路由（主 WebView 存在时按单 WebView 窗口的 `Labeled` 目标派发，子 WebView 仍可收到针对该窗口 label 的监听）和 `Webview::reparent` 的非 `unstable` 拒绝分支（本应用启用 `unstable`，不走此分支）；不含任何 WebView 的窗口改为不视为 WebviewWindow，不影响本应用。无上游测试覆盖，该 crate 不在 workspace 内，未补 Rust 单测。
- 未修补的上游行为：JS `Menu.new` 的内联 action channel 按 MenuId 存于全局表且菜单关闭后不移除。应用层由 `src/components/terminal/NvxTerminalTabBar.vue` 为原生 Tab 菜单及每项使用固定 id，新弹出覆盖并 drop 旧 channel，保留量为常数；新增 JS 原生菜单须沿用同一做法。

### tauri-runtime-wry 2.11.4（`vendor/tauri-runtime-wry-reparent/`）

- 上游：https://github.com/tauri-apps/tauri
- crates.io 基线：`tauri-runtime-wry 2.11.4`；根 Cargo `[patch.crates-io]` 指向本目录。
- 许可证：`Apache-2.0 OR MIT`，正文保留在本目录 `LICENSE_APACHE-2.0`、`LICENSE_MIT`。
- 相对发布包发生修改的文件：`src/lib.rs`。
  - `WebviewMessage::Reparent` 处理：原生 reparent 成功后才把注册项从源窗口移到目标窗口，并在原生调用前释放 `RefCell` 借用；上游先移除源注册，失败或目标窗口缺失时丢失仍存活的 WebView。目标窗口缺失时回复错误，上游不回复导致调用方 panic。
  - `WryWebviewDispatcher::reparent`：回复通道断开时返回 `FailedToSendMessage` 而非 `unwrap` panic。
  - 同窗口 reparent 不做短路，由调用方 `WorkspaceWindows::move_native_view`（`src-tauri/src/workspace_windows.rs`）在 source == target 时直接返回。

### vnc-rs 0.5.3

- 上游：https://github.com/HsuJv/vnc-rs
- crates.io 基线：`vnc-rs 0.5.3`；上游 revision：`ab684d009d767c968af2f7559576334038623124`。
- 许可证：`MIT OR Apache-2.0`，正文保留在本目录对应子目录。
- 相对发布包发生修改的文件（忽略 CRLF/LF 差异）：`Cargo.toml`、`src/client/messages.rs`、`src/client/mod.rs`、`src/codec/mod.rs`、`src/codec/raw.rs`、`src/codec/tight.rs`、`src/codec/zrle.rs`、`src/config.rs`、`src/error.rs`、`src/event.rs`、`src/lib.rs`。
