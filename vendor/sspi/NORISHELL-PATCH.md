# SSPI 0.21.3 依赖兼容补丁

来源：crates.io sspi 0.21.3，upstream commit `09088ac49cf13449656dca94b68f5228919a4d95`。src、tests 及协议实现保持官方发布包原文。

只从 macOS/iOS target 的 Cargo.toml 和 Cargo.toml.orig 删除九项未被 SSPI 源码直接使用的传递版本 pin：curve25519-dalek、ed25519-dalek、p256、p384、p521、primeorder、rustcrypto-ff、rustcrypto-ff_derive、rustcrypto-group。原清单说明这些 pin 应在稳定版本发布后移除；当前稳定版本已存在，且既有 NoriShell SSH 依赖它们。pkcs1/RSA 尚需的预发布 pin 保留。

与 vendor/picky 的 upstream rc.26 稳定依赖修正共同使用。根通过 `[patch.crates-io] sspi = { path = "vendor/sspi" }` 加载；不改现有 russh、NTLM/Kerberos 算法源码或网络行为。NoriShell RDP 本身固定 NTLM 并拒绝额外网络请求。

上游 SSPI 发布包含此依赖整理的版本后可移除此 patch。许可证随源码保留。
