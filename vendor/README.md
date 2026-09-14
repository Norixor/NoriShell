# 第三方源码与本地补丁

这些目录保留各自上游许可证，不适用仓库原创代码的GPL-3.0-only 许可证声明。Cargo 的 `[patch.crates-io]` 与 `crates/vnc-client/Cargo.toml` 指定实际使用的副本；不得直接用 registry 版本替换而丢弃本地改动。

以下清单以相同名称及版本的 crates.io 发布包为比较基线；修改文件带有 NoriShell 标记，原版权声明保留。更新补丁时同步此清单、修改文件标记及相关协议回归测试。需要具体差异时，对照所列发布包与本目录源码；本清单不将上游 revision 冒充本项目提交。

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

### vnc-rs 0.5.3

- 上游：https://github.com/HsuJv/vnc-rs
- crates.io 基线：`vnc-rs 0.5.3`；上游 revision：`ab684d009d767c968af2f7559576334038623124`。
- 许可证：`MIT OR Apache-2.0`，正文保留在本目录对应子目录。
- 相对发布包发生修改的文件（忽略 CRLF/LF 差异）：`Cargo.toml`、`src/client/messages.rs`、`src/client/mod.rs`、`src/codec/mod.rs`、`src/codec/raw.rs`、`src/codec/tight.rs`、`src/codec/zrle.rs`、`src/config.rs`、`src/error.rs`、`src/event.rs`、`src/lib.rs`。
