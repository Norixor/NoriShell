# Picky 依赖兼容补丁

来源：crates.io picky 7.0.0-rc.25，upstream commit `18a3a419adcbe0034f7fdfc694039cc970d53766`。

IronRDP connector 0.10.0 精确要求此版本。其预发布 crypto pins 与现有 russh 的已发布稳定版同 major 要求冲突，Cargo 无法共同解析。

本地保留 7.0.0-rc.25 包标识及全部 src 原文，只采用 upstream 7.0.0-rc.26（commit `f99da8b9a77e56e4cf4e1940dfe3a410365d84c7`）的 Cargo.toml / Cargo.toml.orig 依赖改动：Ed25519/X25519 3、P256/P384/P521 0.14、AES-GCM 0.11 稳定版本，以及删除已经无用的 curve25519/ecdsa/primeorder/rustcrypto 预发布 pin。

已比较两个官方发布包的 src，完全相同；本补丁不改密码算法代码、不改 russh 依赖、不关闭证书或签名校验。根 Cargo 通过 `[patch.crates-io] picky = { path = "vendor/picky" }` 加载。

IronRDP 更新到接受 Picky rc.26 或后续稳定依赖版本后，可以去除此 patch 和 vendored 目录。许可证随源码保留。
