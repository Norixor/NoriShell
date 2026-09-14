//! SSH client transport for one authenticated remote shell channel.
//!
//! This crate deliberately has no knowledge of projects, local working
//! directories, local PTYs, Tauri, or the credential Vault. Callers supply a
//! scoped authentication secret only after an explicit host-key verifier has
//! trusted the server.

mod exec_stream;
mod route_connector;
mod shared_channels;
pub use shared_channels::{
    SharedRemoteForward, SharedSessionChannels, SharedSftpChannel, SharedTerminalChannel,
};

pub use exec_stream::{RemoteExecStream, RemoteExecStreamEvent, RemoteExecStreamLimits};

pub use route_connector::{
    IngressFailureKind, IngressStage, ProxyCredentialError, ProxyCredentials, RouteIngress,
    RouteIngressError, Socks5DnsMode, connect_route_stream,
};

use std::{
    borrow::Cow,
    collections::VecDeque,
    fmt,
    future::Future,
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};

use bytes::Bytes;
use norishell_ssh_domain::{Endpoint, EndpointError, ssh_sha256_fingerprint};
use russh::keys::ssh_key::{Algorithm, EcdsaCurve};
pub use russh::keys::{
    PublicKeyBase64,
    agent::{AgentIdentity, client::AgentClient, client::AgentStream},
    ssh_key::{
        Algorithm as AgentKeyAlgorithm, Certificate as OpenSshCertificate,
        EcdsaCurve as AgentEcdsaCurve, HashAlg as AgentHashAlgorithm, PublicKey as AgentPublicKey,
        certificate::CertType as AgentCertificateType,
    },
};
use russh::{
    Channel, ChannelId, ChannelMsg, ChannelOpenFailure, ChannelReadHalf, ChannelWriteHalf,
    Disconnect, client,
    keys::{PrivateKeyWithHashAlg, decode_secret_key, ssh_key::HashAlg},
};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::{mpsc, watch},
    time::timeout,
};
use zeroize::Zeroizing;

const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(150);
const DEFAULT_HOST_KEY_DECISION_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_AUTHENTICATION_TIMEOUT: Duration = Duration::from_secs(30);
const SSH_AGENT_SIGN_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_CHANNEL_OPEN_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_CHANNEL_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_USERNAME_BYTES: usize = 128;
const MAX_PRIVATE_KEY_BYTES: usize = 1024 * 1024;
const MAX_KEYBOARD_INTERACTIVE_PROMPTS_PER_ROUND: usize = 32;
const MAX_KEYBOARD_INTERACTIVE_TEXT_BYTES: usize = 4 * 1024;
const MAX_KEYBOARD_INTERACTIVE_ROUND_BYTES: usize = 64 * 1024;
const MAX_KEYBOARD_INTERACTIVE_ANSWER_BYTES: usize = 64 * 1024;
const MAX_PENDING_FORWARDED_CHANNELS: usize = 64;
const MAX_FORWARD_ADDRESS_BYTES: usize = 255;

pub const SECURE_DEFAULT_ALGORITHM_POLICY_ID: &str = "secure-default";
pub const ALGORITHM_POLICY_CATALOG_VERSION: &str = "2026.08.29.1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlgorithmCategory {
    KeyExchange,
    HostKey,
    Cipher,
    Mac,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlgorithmRisk {
    Modern,
    Legacy,
    Weak,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlgorithmBackend {
    KexMlkem768X25519Sha256,
    KexCurve25519Sha256,
    KexCurve25519Sha256Libssh,
    KexDhGexSha256,
    KexDhGroup18Sha512,
    KexDhGroup17Sha512,
    KexDhGroup16Sha512,
    KexDhGroup15Sha512,
    KexDhGroup14Sha256,
    KexDhGroup14Sha1,
    HostKeyEd25519,
    HostKeyEcdsaP256,
    HostKeyEcdsaP384,
    HostKeyEcdsaP521,
    HostKeyRsaSha512,
    HostKeyRsaSha256,
    HostKeyRsaSha1,
    CipherChacha20Poly1305,
    CipherAes256Gcm,
    CipherAes128Gcm,
    CipherAes256Ctr,
    CipherAes192Ctr,
    CipherAes128Ctr,
    CipherAes256Cbc,
    CipherAes128Cbc,
    MacHmacSha512Etm,
    MacHmacSha256Etm,
    MacHmacSha512,
    MacHmacSha256,
    MacHmacSha1Etm,
    MacHmacSha1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlgorithmCatalogEntry {
    pub stable_id: &'static str,
    pub category: AlgorithmCategory,
    pub algorithm_name: &'static str,
    pub enabled_by_default: bool,
    pub selectable_exception: bool,
    pub risk: AlgorithmRisk,
    backend: AlgorithmBackend,
}

const ALGORITHM_CATALOG: &[AlgorithmCatalogEntry] = &[
    AlgorithmCatalogEntry {
        stable_id: "kex-mlkem768x25519-sha256",
        backend: AlgorithmBackend::KexMlkem768X25519Sha256,
        category: AlgorithmCategory::KeyExchange,
        algorithm_name: "mlkem768x25519-sha256",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "kex-curve25519-sha256",
        backend: AlgorithmBackend::KexCurve25519Sha256,
        category: AlgorithmCategory::KeyExchange,
        algorithm_name: "curve25519-sha256",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "kex-curve25519-sha256-libssh",
        backend: AlgorithmBackend::KexCurve25519Sha256Libssh,
        category: AlgorithmCategory::KeyExchange,
        algorithm_name: "curve25519-sha256@libssh.org",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "kex-dh-gex-sha256",
        backend: AlgorithmBackend::KexDhGexSha256,
        category: AlgorithmCategory::KeyExchange,
        algorithm_name: "diffie-hellman-group-exchange-sha256",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "kex-dh-group18-sha512",
        backend: AlgorithmBackend::KexDhGroup18Sha512,
        category: AlgorithmCategory::KeyExchange,
        algorithm_name: "diffie-hellman-group18-sha512",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "kex-dh-group17-sha512",
        backend: AlgorithmBackend::KexDhGroup17Sha512,
        category: AlgorithmCategory::KeyExchange,
        algorithm_name: "diffie-hellman-group17-sha512",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "kex-dh-group16-sha512",
        backend: AlgorithmBackend::KexDhGroup16Sha512,
        category: AlgorithmCategory::KeyExchange,
        algorithm_name: "diffie-hellman-group16-sha512",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "kex-dh-group15-sha512",
        backend: AlgorithmBackend::KexDhGroup15Sha512,
        category: AlgorithmCategory::KeyExchange,
        algorithm_name: "diffie-hellman-group15-sha512",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "kex-dh-group14-sha256",
        backend: AlgorithmBackend::KexDhGroup14Sha256,
        category: AlgorithmCategory::KeyExchange,
        algorithm_name: "diffie-hellman-group14-sha256",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "host-key-ed25519",
        backend: AlgorithmBackend::HostKeyEd25519,
        category: AlgorithmCategory::HostKey,
        algorithm_name: "ssh-ed25519",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "host-key-ecdsa-p256",
        backend: AlgorithmBackend::HostKeyEcdsaP256,
        category: AlgorithmCategory::HostKey,
        algorithm_name: "ecdsa-sha2-nistp256",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "host-key-ecdsa-p384",
        backend: AlgorithmBackend::HostKeyEcdsaP384,
        category: AlgorithmCategory::HostKey,
        algorithm_name: "ecdsa-sha2-nistp384",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "host-key-ecdsa-p521",
        backend: AlgorithmBackend::HostKeyEcdsaP521,
        category: AlgorithmCategory::HostKey,
        algorithm_name: "ecdsa-sha2-nistp521",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "host-key-rsa-sha512",
        backend: AlgorithmBackend::HostKeyRsaSha512,
        category: AlgorithmCategory::HostKey,
        algorithm_name: "rsa-sha2-512",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "host-key-rsa-sha256",
        backend: AlgorithmBackend::HostKeyRsaSha256,
        category: AlgorithmCategory::HostKey,
        algorithm_name: "rsa-sha2-256",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "cipher-chacha20-poly1305",
        backend: AlgorithmBackend::CipherChacha20Poly1305,
        category: AlgorithmCategory::Cipher,
        algorithm_name: "chacha20-poly1305@openssh.com",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "cipher-aes256-gcm",
        backend: AlgorithmBackend::CipherAes256Gcm,
        category: AlgorithmCategory::Cipher,
        algorithm_name: "aes256-gcm@openssh.com",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "cipher-aes128-gcm",
        backend: AlgorithmBackend::CipherAes128Gcm,
        category: AlgorithmCategory::Cipher,
        algorithm_name: "aes128-gcm@openssh.com",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "cipher-aes256-ctr",
        backend: AlgorithmBackend::CipherAes256Ctr,
        category: AlgorithmCategory::Cipher,
        algorithm_name: "aes256-ctr",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "cipher-aes192-ctr",
        backend: AlgorithmBackend::CipherAes192Ctr,
        category: AlgorithmCategory::Cipher,
        algorithm_name: "aes192-ctr",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "cipher-aes128-ctr",
        backend: AlgorithmBackend::CipherAes128Ctr,
        category: AlgorithmCategory::Cipher,
        algorithm_name: "aes128-ctr",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "mac-hmac-sha512-etm",
        backend: AlgorithmBackend::MacHmacSha512Etm,
        category: AlgorithmCategory::Mac,
        algorithm_name: "hmac-sha2-512-etm@openssh.com",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "mac-hmac-sha256-etm",
        backend: AlgorithmBackend::MacHmacSha256Etm,
        category: AlgorithmCategory::Mac,
        algorithm_name: "hmac-sha2-256-etm@openssh.com",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "mac-hmac-sha512",
        backend: AlgorithmBackend::MacHmacSha512,
        category: AlgorithmCategory::Mac,
        algorithm_name: "hmac-sha2-512",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "mac-hmac-sha256",
        backend: AlgorithmBackend::MacHmacSha256,
        category: AlgorithmCategory::Mac,
        algorithm_name: "hmac-sha2-256",
        enabled_by_default: true,
        selectable_exception: false,
        risk: AlgorithmRisk::Modern,
    },
    AlgorithmCatalogEntry {
        stable_id: "compat-kex-dh-group14-sha1",
        backend: AlgorithmBackend::KexDhGroup14Sha1,
        category: AlgorithmCategory::KeyExchange,
        algorithm_name: "diffie-hellman-group14-sha1",
        enabled_by_default: false,
        selectable_exception: true,
        risk: AlgorithmRisk::Weak,
    },
    AlgorithmCatalogEntry {
        stable_id: "compat-host-key-rsa-sha1",
        backend: AlgorithmBackend::HostKeyRsaSha1,
        category: AlgorithmCategory::HostKey,
        algorithm_name: "ssh-rsa",
        enabled_by_default: false,
        selectable_exception: true,
        risk: AlgorithmRisk::Weak,
    },
    AlgorithmCatalogEntry {
        stable_id: "compat-cipher-aes256-cbc",
        backend: AlgorithmBackend::CipherAes256Cbc,
        category: AlgorithmCategory::Cipher,
        algorithm_name: "aes256-cbc",
        enabled_by_default: false,
        selectable_exception: true,
        risk: AlgorithmRisk::Legacy,
    },
    AlgorithmCatalogEntry {
        stable_id: "compat-cipher-aes128-cbc",
        backend: AlgorithmBackend::CipherAes128Cbc,
        category: AlgorithmCategory::Cipher,
        algorithm_name: "aes128-cbc",
        enabled_by_default: false,
        selectable_exception: true,
        risk: AlgorithmRisk::Legacy,
    },
    AlgorithmCatalogEntry {
        stable_id: "compat-mac-hmac-sha1-etm",
        backend: AlgorithmBackend::MacHmacSha1Etm,
        category: AlgorithmCategory::Mac,
        algorithm_name: "hmac-sha1-etm@openssh.com",
        enabled_by_default: false,
        selectable_exception: true,
        risk: AlgorithmRisk::Weak,
    },
    AlgorithmCatalogEntry {
        stable_id: "compat-mac-hmac-sha1",
        backend: AlgorithmBackend::MacHmacSha1,
        category: AlgorithmCategory::Mac,
        algorithm_name: "hmac-sha1",
        enabled_by_default: false,
        selectable_exception: true,
        risk: AlgorithmRisk::Weak,
    },
];

#[must_use]
pub const fn algorithm_policy_catalog() -> &'static [AlgorithmCatalogEntry] {
    ALGORITHM_CATALOG
}

impl AlgorithmBackend {
    fn append(self, preferred: &mut russh::Preferred) {
        match self {
            Self::KexMlkem768X25519Sha256 => preferred
                .kex
                .to_mut()
                .push(russh::kex::MLKEM768X25519_SHA256),
            Self::KexCurve25519Sha256 => preferred.kex.to_mut().push(russh::kex::CURVE25519),
            Self::KexCurve25519Sha256Libssh => preferred
                .kex
                .to_mut()
                .push(russh::kex::CURVE25519_PRE_RFC_8731),
            Self::KexDhGexSha256 => preferred.kex.to_mut().push(russh::kex::DH_GEX_SHA256),
            Self::KexDhGroup18Sha512 => preferred.kex.to_mut().push(russh::kex::DH_G18_SHA512),
            Self::KexDhGroup17Sha512 => preferred.kex.to_mut().push(russh::kex::DH_G17_SHA512),
            Self::KexDhGroup16Sha512 => preferred.kex.to_mut().push(russh::kex::DH_G16_SHA512),
            Self::KexDhGroup15Sha512 => preferred.kex.to_mut().push(russh::kex::DH_G15_SHA512),
            Self::KexDhGroup14Sha256 => preferred.kex.to_mut().push(russh::kex::DH_G14_SHA256),
            Self::KexDhGroup14Sha1 => preferred.kex.to_mut().push(russh::kex::DH_G14_SHA1),
            Self::HostKeyEd25519 => preferred.key.to_mut().push(Algorithm::Ed25519),
            Self::HostKeyEcdsaP256 => preferred.key.to_mut().push(Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP256,
            }),
            Self::HostKeyEcdsaP384 => preferred.key.to_mut().push(Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP384,
            }),
            Self::HostKeyEcdsaP521 => preferred.key.to_mut().push(Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP521,
            }),
            Self::HostKeyRsaSha512 => preferred.key.to_mut().push(Algorithm::Rsa {
                hash: Some(HashAlg::Sha512),
            }),
            Self::HostKeyRsaSha256 => preferred.key.to_mut().push(Algorithm::Rsa {
                hash: Some(HashAlg::Sha256),
            }),
            Self::HostKeyRsaSha1 => preferred.key.to_mut().push(Algorithm::Rsa { hash: None }),
            Self::CipherChacha20Poly1305 => preferred
                .cipher
                .to_mut()
                .push(russh::cipher::CHACHA20_POLY1305),
            Self::CipherAes256Gcm => preferred.cipher.to_mut().push(russh::cipher::AES_256_GCM),
            Self::CipherAes128Gcm => preferred.cipher.to_mut().push(russh::cipher::AES_128_GCM),
            Self::CipherAes256Ctr => preferred.cipher.to_mut().push(russh::cipher::AES_256_CTR),
            Self::CipherAes192Ctr => preferred.cipher.to_mut().push(russh::cipher::AES_192_CTR),
            Self::CipherAes128Ctr => preferred.cipher.to_mut().push(russh::cipher::AES_128_CTR),
            Self::CipherAes256Cbc => preferred.cipher.to_mut().push(russh::cipher::AES_256_CBC),
            Self::CipherAes128Cbc => preferred.cipher.to_mut().push(russh::cipher::AES_128_CBC),
            Self::MacHmacSha512Etm => preferred.mac.to_mut().push(russh::mac::HMAC_SHA512_ETM),
            Self::MacHmacSha256Etm => preferred.mac.to_mut().push(russh::mac::HMAC_SHA256_ETM),
            Self::MacHmacSha512 => preferred.mac.to_mut().push(russh::mac::HMAC_SHA512),
            Self::MacHmacSha256 => preferred.mac.to_mut().push(russh::mac::HMAC_SHA256),
            Self::MacHmacSha1Etm => preferred.mac.to_mut().push(russh::mac::HMAC_SHA1_ETM),
            Self::MacHmacSha1 => preferred.mac.to_mut().push(russh::mac::HMAC_SHA1),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AlgorithmPolicyError {
    #[error("unknown algorithm policy")]
    UnknownPolicy,
    #[error("unknown or unavailable algorithm compatibility exception")]
    UnknownException,
    #[error("algorithm compatibility exception category does not match the catalog")]
    CategoryMismatch,
    #[error("algorithm compatibility exception is duplicated")]
    DuplicateException,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlgorithmPolicy {
    exception_ids: Vec<&'static str>,
}

impl AlgorithmPolicy {
    #[must_use]
    pub const fn secure_default() -> Self {
        Self {
            exception_ids: Vec::new(),
        }
    }

    pub fn from_catalog_ids<'a>(
        policy_id: &str,
        exceptions: impl IntoIterator<Item = (AlgorithmCategory, &'a str)>,
    ) -> std::result::Result<Self, AlgorithmPolicyError> {
        if policy_id != SECURE_DEFAULT_ALGORITHM_POLICY_ID {
            return Err(AlgorithmPolicyError::UnknownPolicy);
        }
        let mut exception_ids = Vec::new();
        for (category, stable_id) in exceptions {
            let entry = ALGORITHM_CATALOG
                .iter()
                .find(|entry| entry.stable_id == stable_id && entry.selectable_exception)
                .ok_or(AlgorithmPolicyError::UnknownException)?;
            if entry.category != category {
                return Err(AlgorithmPolicyError::CategoryMismatch);
            }
            if exception_ids.contains(&entry.stable_id) {
                return Err(AlgorithmPolicyError::DuplicateException);
            }
            exception_ids.push(entry.stable_id);
        }
        Ok(Self { exception_ids })
    }

    fn preferred(&self) -> russh::Preferred {
        let mut preferred = russh::Preferred {
            kex: Cow::Owned(Vec::new()),
            key: Cow::Owned(Vec::new()),
            host_key_certificates: Cow::Borrowed(&[]),
            cipher: Cow::Owned(Vec::new()),
            mac: Cow::Owned(Vec::new()),
            compression: Cow::Owned(vec![russh::compression::NONE]),
        };
        for entry in ALGORITHM_CATALOG.iter().filter(|entry| {
            entry.enabled_by_default || self.exception_ids.contains(&entry.stable_id)
        }) {
            entry.backend.append(&mut preferred);
        }
        // Protocol capability markers are mandatory implementation details,
        // not user-configurable algorithms, and therefore are not catalog rows.
        preferred.kex.to_mut().extend([
            russh::kex::EXTENSION_SUPPORT_AS_CLIENT,
            russh::kex::EXTENSION_OPENSSH_STRICT_KEX_AS_CLIENT,
        ]);
        preferred
    }
}

pub type VerifyFuture = Pin<
    Box<dyn Future<Output = std::result::Result<HostKeyDecision, TransportError>> + Send + 'static>,
>;

/// Application policy invoked only after SSH has verified the KEX signature.
///
/// The verifier must base trust on the user-supplied normalized endpoint, not
/// on a DNS-resolved IP address. Returning `Trusted` is the only path that lets
/// authentication continue.
pub trait HostKeyVerifier: Send + Sync + 'static {
    fn verify(&self, endpoint: Endpoint, observed: ObservedHostKey) -> VerifyFuture;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedHostKey {
    pub algorithm: String,
    pub public_key_blob: Vec<u8>,
    pub fingerprint_sha256: String,
}

/// Secret-free algorithms selected by the SSH handshake. Cipher and MAC are
/// negotiated independently in each direction and remain explicit at the
/// application boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegotiatedAlgorithms {
    pub key_exchange: String,
    pub host_key: String,
    pub cipher_client_to_server: String,
    pub cipher_server_to_client: String,
    pub mac_client_to_server: String,
    pub mac_server_to_client: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyDecision {
    Trusted,
    Rejected,
    Mismatch { trusted_fingerprint: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtySize {
    columns: u32,
    rows: u32,
    pixel_width: u32,
    pixel_height: u32,
}

impl PtySize {
    pub fn new(columns: u32, rows: u32, pixel_width: u32, pixel_height: u32) -> Result<Self> {
        if columns == 0 || rows == 0 {
            return Err(TransportError::InvalidPtySize);
        }
        Ok(Self {
            columns,
            rows,
            pixel_width,
            pixel_height,
        })
    }

    fn validate(self) -> Result<Self> {
        Self::new(self.columns, self.rows, self.pixel_width, self.pixel_height)
    }

    #[must_use]
    pub const fn columns(self) -> u32 {
        self.columns
    }

    #[must_use]
    pub const fn rows(self) -> u32 {
        self.rows
    }

    #[must_use]
    pub const fn pixel_width(self) -> u32 {
        self.pixel_width
    }

    #[must_use]
    pub const fn pixel_height(self) -> u32 {
        self.pixel_height
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectionTimeouts {
    pub tcp_connect: Duration,
    pub handshake: Duration,
    pub host_key_decision: Duration,
    pub authentication: Duration,
    pub channel_open: Duration,
    pub channel_request: Duration,
    pub disconnect: Duration,
}

impl Default for ConnectionTimeouts {
    fn default() -> Self {
        Self {
            tcp_connect: DEFAULT_CONNECT_TIMEOUT,
            handshake: DEFAULT_HANDSHAKE_TIMEOUT,
            host_key_decision: DEFAULT_HOST_KEY_DECISION_TIMEOUT,
            authentication: DEFAULT_AUTHENTICATION_TIMEOUT,
            channel_open: DEFAULT_CHANNEL_OPEN_TIMEOUT,
            channel_request: DEFAULT_CHANNEL_REQUEST_TIMEOUT,
            disconnect: DEFAULT_DISCONNECT_TIMEOUT,
        }
    }
}

pub struct ConnectRequest {
    endpoint: Endpoint,
    ingress: RouteIngress,
    timeouts: ConnectionTimeouts,
    algorithm_policy: AlgorithmPolicy,
}

impl fmt::Debug for ConnectRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectRequest")
            .field("endpoint", &self.endpoint)
            .field("ingress", &self.ingress)
            .field("timeouts", &self.timeouts)
            .field("algorithm_policy", &self.algorithm_policy)
            .finish()
    }
}

impl ConnectRequest {
    pub fn new(address: &str, port: u16) -> Result<Self> {
        Ok(Self {
            endpoint: Endpoint::parse(address, port)?,
            ingress: RouteIngress::DirectTcp,
            timeouts: ConnectionTimeouts::default(),
            algorithm_policy: AlgorithmPolicy::secure_default(),
        })
    }

    /// Selects the local route ingress. `ConnectRequest::new` remains Direct
    /// TCP by default for source and behavior compatibility.
    #[must_use]
    pub fn with_ingress(mut self, ingress: RouteIngress) -> Self {
        self.ingress = ingress;
        self
    }

    #[must_use]
    pub fn with_algorithm_policy(mut self, algorithm_policy: AlgorithmPolicy) -> Self {
        self.algorithm_policy = algorithm_policy;
        self
    }

    #[must_use]
    pub fn with_timeouts(mut self, timeouts: ConnectionTimeouts) -> Self {
        self.timeouts = timeouts;
        self
    }
}

pub enum Authentication {
    Password {
        password: Zeroizing<Vec<u8>>,
    },
    PrivateKey {
        private_key: Zeroizing<Vec<u8>>,
        passphrase: Option<Zeroizing<Vec<u8>>>,
    },
    SshAgent {
        identity: Box<AgentIdentity>,
        client: AgentClient<Box<dyn AgentStream + Send + Unpin + 'static>>,
    },
    SshAgentCertificate {
        certificate: Box<OpenSshCertificate>,
        client: AgentClient<Box<dyn AgentStream + Send + Unpin + 'static>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyboardInteractivePrompt {
    pub text: String,
    pub echo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyboardInteractiveChallenge {
    pub name: String,
    pub instructions: String,
    pub prompts: Vec<KeyboardInteractivePrompt>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyboardInteractiveOutcome {
    Success,
    Rejected {
        remaining_methods: Vec<String>,
        partial_success: bool,
    },
    Challenge(KeyboardInteractiveChallenge),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivateKeyMetadata {
    pub algorithm: String,
    pub fingerprint_sha256: String,
}

/// Validates copied private-key material before it is committed to the Vault
/// and derives the only public metadata persisted in SQLite.
pub fn inspect_private_key(
    private_key: &[u8],
    passphrase: Option<&[u8]>,
) -> Result<PrivateKeyMetadata> {
    if private_key.is_empty() || private_key.len() > MAX_PRIVATE_KEY_BYTES {
        return Err(TransportError::InvalidPrivateKey);
    }
    let private_key =
        std::str::from_utf8(private_key).map_err(|_| TransportError::InvalidPrivateKey)?;
    let passphrase = passphrase
        .map(std::str::from_utf8)
        .transpose()
        .map_err(|_| TransportError::InvalidPrivateKey)?;
    let key = decode_secret_key(private_key, passphrase)
        .map_err(|_| TransportError::InvalidPrivateKey)?;
    let public_key = key.public_key();
    Ok(PrivateKeyMetadata {
        algorithm: public_key.algorithm().to_string(),
        fingerprint_sha256: ssh_sha256_fingerprint(&public_key.public_key_bytes()),
    })
}

impl Authentication {
    #[must_use]
    pub fn password(password: Vec<u8>) -> Self {
        Self::Password {
            password: Zeroizing::new(password),
        }
    }

    #[must_use]
    pub fn private_key(private_key: Vec<u8>, passphrase: Option<Vec<u8>>) -> Self {
        Self::PrivateKey {
            private_key: Zeroizing::new(private_key),
            passphrase: passphrase.map(Zeroizing::new),
        }
    }

    #[must_use]
    pub fn ssh_agent<S>(identity: AgentIdentity, client: AgentClient<S>) -> Self
    where
        S: AgentStream + Send + Unpin + 'static,
    {
        Self::SshAgent {
            identity: Box::new(identity),
            client: client.dynamic(),
        }
    }

    #[must_use]
    pub fn ssh_agent_certificate<S>(certificate: OpenSshCertificate, client: AgentClient<S>) -> Self
    where
        S: AgentStream + Send + Unpin + 'static,
    {
        Self::SshAgentCertificate {
            certificate: Box::new(certificate),
            client: client.dynamic(),
        }
    }

    const fn timeout(&self, default: Duration) -> Duration {
        match self {
            Self::SshAgent { .. } | Self::SshAgentCertificate { .. } => SSH_AGENT_SIGN_TIMEOUT,
            Self::Password { .. } | Self::PrivateKey { .. } => default,
        }
    }
}

impl fmt::Debug for Authentication {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Password { .. } => "Password",
            Self::PrivateKey { .. } => "PrivateKey",
            Self::SshAgent { .. } => "SshAgent",
            Self::SshAgentCertificate { .. } => "SshAgentCertificate",
        };
        formatter
            .debug_struct("Authentication")
            .field("kind", &kind)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error(transparent)]
    InvalidEndpoint(#[from] EndpointError),
    #[error("SSH username is empty or invalid")]
    InvalidUsername,
    #[error("remote PTY dimensions must be non-zero")]
    InvalidPtySize,
    #[error("SSH TCP connection timed out")]
    ConnectTimeout,
    #[error("SSH TCP connection failed")]
    ConnectFailed,
    #[error(transparent)]
    RouteIngress(#[from] RouteIngressError),
    #[error(transparent)]
    Jump(#[from] JumpTransportError),
    #[error("server host key was rejected")]
    HostKeyRejected,
    #[error("server host key does not match the trusted key")]
    HostKeyMismatch {
        trusted_fingerprint: String,
        observed_fingerprint: String,
    },
    #[error("host-key verification failed")]
    HostKeyVerificationFailed,
    #[error("SSH handshake timed out")]
    HandshakeTimeout,
    #[error("host-key decision timed out")]
    HostKeyDecisionTimeout,
    #[error("SSH authentication timed out")]
    AuthenticationTimeout,
    #[error("SSH authentication was rejected")]
    AuthenticationRejected,
    #[error("SSH authentication stopped after partial success and requires another factor")]
    AuthenticationIncomplete,
    #[error("keyboard-interactive challenge or response is invalid or exceeds limits")]
    InvalidKeyboardInteractiveResponse,
    #[error("the selected SSH Agent key is unavailable")]
    SshAgentKeyUnavailable,
    #[error("the SSH Agent is unavailable or failed to sign")]
    SshAgentUnavailable,
    #[error("SSH private key material is invalid or cannot be decrypted")]
    InvalidPrivateKey,
    #[error("server only supports the legacy RSA/SHA-1 signature algorithm")]
    InsecureRsaSignatureOnly,
    #[error("opening the SSH session channel timed out")]
    ChannelOpenTimeout,
    #[error("server rejected the remote PTY request")]
    PtyRejected,
    #[error("remote PTY request timed out")]
    PtyRequestTimeout,
    #[error("server rejected the remote shell request")]
    ShellRejected,
    #[error("remote shell request timed out")]
    ShellRequestTimeout,
    #[error("remote exec command is empty or too large")]
    InvalidRemoteExecCommand,
    #[error("remote exec command timed out")]
    RemoteExecTimeout,
    #[error("remote exec command was rejected")]
    RemoteExecRejected,
    #[error("remote exec output exceeded the bounded capture limit")]
    RemoteExecOutputTooLarge,
    #[error("SSH port forwarding request timed out")]
    ForwardRequestTimeout,
    #[error("server rejected the SSH port forwarding request")]
    ForwardRequestRejected,
    #[error("opening an SSH forwarded channel timed out")]
    ForwardChannelOpenTimeout,
    #[error("server rejected the SSH forwarded channel")]
    ForwardChannelOpenRejected,
    #[error("opening the SFTP subsystem timed out")]
    SftpSubsystemTimeout,
    #[error("server rejected the SFTP subsystem")]
    SftpSubsystemRejected,
    #[error("SFTP protocol initialization failed")]
    SftpProtocol,
    #[error("SSH connection was lost")]
    ConnectionLost,
    #[error("SSH disconnect timed out")]
    DisconnectTimeout,
    #[error("no common SSH algorithm for {category:?}")]
    AlgorithmNegotiationFailed {
        category: AlgorithmCategory,
        client_candidates: Vec<String>,
        server_candidates: Vec<String>,
    },
    #[error("SSH protocol operation failed")]
    Protocol,
}

impl From<ProxyCredentialError> for TransportError {
    fn from(_: ProxyCredentialError) -> Self {
        Self::RouteIngress(RouteIngressError {
            stage: IngressStage::Configuration,
            kind: IngressFailureKind::InvalidConfiguration,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JumpStage {
    DirectTcpipOpen,
}

impl fmt::Display for JumpStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::DirectTcpipOpen => "jump direct-tcpip open",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JumpFailureKind {
    InvalidConfiguration,
    Timeout,
    Rejected,
    Protocol,
}

impl fmt::Display for JumpFailureKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "invalid configuration",
            Self::Timeout => "timed out",
            Self::Rejected => "server rejected the channel",
            Self::Protocol => "SSH protocol operation failed",
        })
    }
}

/// Credential-free failure for one jump channel stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JumpTransportError {
    pub stage: JumpStage,
    pub kind: JumpFailureKind,
}

impl JumpTransportError {
    const fn new(stage: JumpStage, kind: JumpFailureKind) -> Self {
        Self { stage, kind }
    }
}

impl fmt::Display for JumpTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}", self.stage, self.kind)
    }
}

impl std::error::Error for JumpTransportError {}

pub type Result<T> = std::result::Result<T, TransportError>;

impl From<russh::Error> for TransportError {
    fn from(error: russh::Error) -> Self {
        match error {
            russh::Error::NoCommonAlgo { kind, ours, theirs } => {
                let category = match kind {
                    russh::AlgorithmKind::Kex => AlgorithmCategory::KeyExchange,
                    russh::AlgorithmKind::Key => AlgorithmCategory::HostKey,
                    russh::AlgorithmKind::Cipher => AlgorithmCategory::Cipher,
                    russh::AlgorithmKind::Mac => AlgorithmCategory::Mac,
                    russh::AlgorithmKind::Compression => return Self::Protocol,
                };
                Self::AlgorithmNegotiationFailed {
                    category,
                    client_candidates: bounded_algorithm_candidates(ours),
                    server_candidates: bounded_algorithm_candidates(theirs),
                }
            }
            _ => Self::Protocol,
        }
    }
}

fn bounded_algorithm_candidates(candidates: Vec<String>) -> Vec<String> {
    let mut bounded = Vec::new();
    for candidate in candidates {
        if bounded.len() == 32 {
            break;
        }
        if candidate.is_empty()
            || candidate.len() > 80
            || !candidate.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'@' | b'+')
            })
            || bounded.contains(&candidate)
        {
            continue;
        }
        bounded.push(candidate);
    }
    bounded
}

struct ClientHandler<V> {
    endpoint: Endpoint,
    verifier: Arc<V>,
    host_key_decision_timeout: Duration,
    negotiated_algorithms: Arc<Mutex<Option<NegotiatedAlgorithms>>>,
    forwarded_tcpip_tx: mpsc::Sender<ForwardedTcpipChannel>,
    remote_forward_binding: Arc<Mutex<Option<RemoteForwardBinding>>>,
    shared_remote_forwards: Arc<Mutex<shared_channels::SharedRemoteRouter>>,
    transport_closed: watch::Sender<bool>,
}

impl<V> Drop for ClientHandler<V> {
    fn drop(&mut self) {
        self.transport_closed.send_replace(true);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RemoteForwardBinding {
    address: String,
    port: u16,
    active: bool,
}

fn validated_forwarded_tcpip_metadata(
    binding: Option<&RemoteForwardBinding>,
    connected_address: &str,
    connected_port: u32,
    originator_address: &str,
    originator_port: u32,
) -> Option<(u16, u16)> {
    let binding = binding?;
    let connected_port = u16::try_from(connected_port).ok()?;
    let originator_port = u16::try_from(originator_port).ok()?;
    (binding.active
        && binding.address == connected_address
        && binding.port == connected_port
        && !connected_address.is_empty()
        && connected_address.len() <= MAX_FORWARD_ADDRESS_BYTES
        && !originator_address.is_empty()
        && originator_address.len() <= MAX_FORWARD_ADDRESS_BYTES)
        .then_some((connected_port, originator_port))
}

/// An SSH transport whose server identity has been explicitly trusted.
///
/// This is the first phase that may be given authentication material. It does
/// not expose channel operations, so callers cannot use it as an authenticated
/// transport or open a remote shell before authentication succeeds.
pub struct VerifiedTransport<V>
where
    V: HostKeyVerifier,
{
    session: client::Handle<ClientHandler<V>>,
    timeouts: ConnectionTimeouts,
    authenticated: bool,
    keyboard_interactive_active: bool,
    negotiated_algorithms: Arc<Mutex<Option<NegotiatedAlgorithms>>>,
    forwarded_tcpip_rx: mpsc::Receiver<ForwardedTcpipChannel>,
    remote_forward_binding: Arc<Mutex<Option<RemoteForwardBinding>>>,
    shared_remote_forwards: Arc<Mutex<shared_channels::SharedRemoteRouter>>,
    transport_close_observer: watch::Receiver<bool>,
}

impl<V> fmt::Debug for VerifiedTransport<V>
where
    V: HostKeyVerifier,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedTransport")
            .finish_non_exhaustive()
    }
}

impl<V> VerifiedTransport<V>
where
    V: HostKeyVerifier,
{
    /// Opens TCP, completes the SSH handshake, and verifies the server key.
    ///
    /// Authentication material is deliberately absent from this API so a
    /// pending host-key decision cannot retain a password, private key, or
    /// passphrase.
    pub async fn connect(request: ConnectRequest, verifier: Arc<V>) -> Result<Self> {
        let ConnectRequest {
            endpoint,
            ingress,
            timeouts,
            algorithm_policy,
        } = request;
        let direct = matches!(ingress, RouteIngress::DirectTcp);
        let socket = connect_route_stream(&endpoint, &ingress, timeouts.tcp_connect)
            .await
            .map_err(|error| {
                if direct {
                    match error.kind {
                        IngressFailureKind::Timeout => TransportError::ConnectTimeout,
                        _ => TransportError::ConnectFailed,
                    }
                } else {
                    TransportError::RouteIngress(error)
                }
            })?;

        Self::finish_stream_handshake(endpoint, timeouts, algorithm_policy, socket, verifier).await
    }

    /// Completes a fresh SSH handshake and host-key decision over an already
    /// established byte stream, such as a parent hop's `direct-tcpip` channel.
    ///
    /// The request must retain its default Direct ingress because this method
    /// does not execute another local proxy ingress. Every invocation creates
    /// an independent russh session, algorithm negotiation and verifier call.
    pub async fn connect_stream<S>(
        request: ConnectRequest,
        stream: S,
        verifier: Arc<V>,
    ) -> Result<Self>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let ConnectRequest {
            endpoint,
            ingress,
            timeouts,
            algorithm_policy,
        } = request;
        if !matches!(ingress, RouteIngress::DirectTcp) {
            return Err(TransportError::RouteIngress(RouteIngressError {
                stage: IngressStage::Configuration,
                kind: IngressFailureKind::InvalidConfiguration,
            }));
        }
        Self::finish_stream_handshake(endpoint, timeouts, algorithm_policy, stream, verifier).await
    }

    async fn finish_stream_handshake<S>(
        endpoint: Endpoint,
        timeouts: ConnectionTimeouts,
        algorithm_policy: AlgorithmPolicy,
        stream: S,
        verifier: Arc<V>,
    ) -> Result<Self>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let config = Arc::new(client_config(&algorithm_policy));
        let negotiated_algorithms = Arc::new(Mutex::new(None));
        let (forwarded_tcpip_tx, forwarded_tcpip_rx) =
            mpsc::channel(MAX_PENDING_FORWARDED_CHANNELS);
        let remote_forward_binding = Arc::new(Mutex::new(None));
        let shared_remote_forwards =
            Arc::new(Mutex::new(shared_channels::SharedRemoteRouter::default()));
        let (transport_closed, transport_close_observer) = watch::channel(false);
        let handler = ClientHandler {
            endpoint,
            verifier,
            host_key_decision_timeout: timeouts.host_key_decision,
            negotiated_algorithms: Arc::clone(&negotiated_algorithms),
            forwarded_tcpip_tx,
            remote_forward_binding: Arc::clone(&remote_forward_binding),
            shared_remote_forwards: Arc::clone(&shared_remote_forwards),
            transport_closed,
        };
        let session = timeout(
            timeouts.handshake,
            client::connect_stream(config, stream, handler),
        )
        .await
        .map_err(|_| TransportError::HandshakeTimeout)??;

        Ok(Self {
            session,
            timeouts,
            authenticated: false,
            keyboard_interactive_active: false,
            negotiated_algorithms,
            forwarded_tcpip_rx,
            remote_forward_binding,
            shared_remote_forwards,
            transport_close_observer,
        })
    }

    #[must_use]
    pub fn negotiated_algorithms(&self) -> Option<NegotiatedAlgorithms> {
        self.negotiated_algorithms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Consumes a verified transport and authenticates exactly once.
    ///
    /// The authentication value is consumed inside this call and is not stored
    /// in the returned transport.
    pub async fn authenticate(
        mut self,
        username: impl Into<String>,
        authentication: Authentication,
    ) -> Result<AuthenticatedTransport<V>> {
        self.authenticate_attempt(username, authentication).await?;
        self.into_authenticated()
    }

    /// Attempts one bounded authentication method on the already host-key-verified connection.
    /// A rejection leaves the transport available for the next configured method; protocol or
    /// timeout failures still abort the connection.
    pub async fn authenticate_attempt(
        &mut self,
        username: impl Into<String>,
        authentication: Authentication,
    ) -> Result<()> {
        if self.authenticated || self.keyboard_interactive_active {
            return Err(TransportError::Protocol);
        }
        let username = username.into();
        validate_username(&username)?;
        let authentication_timeout = authentication.timeout(self.timeouts.authentication);
        timeout(
            authentication_timeout,
            authenticate(&mut self.session, username, authentication),
        )
        .await
        .map_err(|_| TransportError::AuthenticationTimeout)??;
        self.authenticated = true;
        Ok(())
    }

    pub async fn keyboard_interactive_start(
        &mut self,
        username: impl Into<String>,
    ) -> Result<KeyboardInteractiveOutcome> {
        if self.authenticated || self.keyboard_interactive_active {
            return Err(TransportError::Protocol);
        }
        let username = username.into();
        validate_username(&username)?;
        let response = timeout(
            self.timeouts.authentication,
            self.session
                .authenticate_keyboard_interactive_start(username, None),
        )
        .await
        .map_err(|_| TransportError::AuthenticationTimeout)?
        .map_err(map_protocol_error)?;
        self.apply_keyboard_interactive_response(response)
    }

    pub async fn keyboard_interactive_respond(
        &mut self,
        responses: Vec<Zeroizing<String>>,
    ) -> Result<KeyboardInteractiveOutcome> {
        if self.authenticated || !self.keyboard_interactive_active {
            return Err(TransportError::Protocol);
        }
        if responses
            .iter()
            .any(|response| response.len() > MAX_KEYBOARD_INTERACTIVE_ANSWER_BYTES)
        {
            self.keyboard_interactive_active = false;
            return Err(TransportError::InvalidKeyboardInteractiveResponse);
        }
        // Russh currently accepts owned String values. This creates one scoped
        // dependency-side copy; the caller-owned values remain zeroized on drop.
        let dependency_responses = responses
            .iter()
            .map(|response| response.as_str().to_owned())
            .collect();
        let response = timeout(
            self.timeouts.authentication,
            self.session
                .authenticate_keyboard_interactive_respond(dependency_responses),
        )
        .await
        .map_err(|_| TransportError::AuthenticationTimeout)?
        .map_err(map_protocol_error)?;
        self.apply_keyboard_interactive_response(response)
    }

    fn apply_keyboard_interactive_response(
        &mut self,
        response: client::KeyboardInteractiveAuthResponse,
    ) -> Result<KeyboardInteractiveOutcome> {
        match response {
            client::KeyboardInteractiveAuthResponse::Success => {
                self.keyboard_interactive_active = false;
                self.authenticated = true;
                Ok(KeyboardInteractiveOutcome::Success)
            }
            client::KeyboardInteractiveAuthResponse::Failure {
                remaining_methods,
                partial_success,
            } => {
                self.keyboard_interactive_active = false;
                Ok(KeyboardInteractiveOutcome::Rejected {
                    remaining_methods: remaining_methods.iter().map(String::from).collect(),
                    partial_success,
                })
            }
            client::KeyboardInteractiveAuthResponse::InfoRequest {
                name,
                instructions,
                prompts,
            } => {
                let challenge =
                    validated_keyboard_interactive_challenge(name, instructions, prompts)?;
                self.keyboard_interactive_active = true;
                Ok(KeyboardInteractiveOutcome::Challenge(challenge))
            }
        }
    }

    pub fn into_authenticated(self) -> Result<AuthenticatedTransport<V>> {
        let Self {
            session,
            timeouts,
            authenticated,
            forwarded_tcpip_rx,
            remote_forward_binding,
            shared_remote_forwards,
            transport_close_observer,
            ..
        } = self;
        if !authenticated {
            return Err(TransportError::AuthenticationRejected);
        }
        Ok(AuthenticatedTransport {
            session,
            timeouts,
            forwarded_tcpip_rx,
            remote_forward_binding,
            shared_remote_forwards,
            transport_close_observer,
        })
    }
}

/// A host-key-verified and authenticated SSH transport.
///
/// This phase no longer retains the authentication value and can be consumed
/// to open one interactive remote shell channel.
pub struct AuthenticatedTransport<V>
where
    V: HostKeyVerifier,
{
    session: client::Handle<ClientHandler<V>>,
    timeouts: ConnectionTimeouts,
    forwarded_tcpip_rx: mpsc::Receiver<ForwardedTcpipChannel>,
    remote_forward_binding: Arc<Mutex<Option<RemoteForwardBinding>>>,
    shared_remote_forwards: Arc<Mutex<shared_channels::SharedRemoteRouter>>,
    transport_close_observer: watch::Receiver<bool>,
}

/// Passive, least-privilege observation of the owning SSH transport closing.
///
/// Waiting on this handle never sends protocol traffic. It is therefore safe
/// for resources whose heartbeat policy is Disabled or ShellHeartbeat.
#[derive(Clone)]
pub struct TransportCloseHandle {
    closed: watch::Receiver<bool>,
}

impl fmt::Debug for TransportCloseHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TransportCloseHandle")
            .field("closed", &self.is_closed())
            .finish()
    }
}

impl TransportCloseHandle {
    #[must_use]
    pub fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    /// Resolves after the SSH session ends, including an idle remote close.
    pub async fn wait_closed(&mut self) {
        if self.is_closed() {
            return;
        }
        while self.closed.changed().await.is_ok() {
            if self.is_closed() {
                return;
            }
        }
    }
}

/// Least-privilege protocol heartbeat capability for one authenticated SSH
/// transport. Clones refer to the same transport and do not expose channels.
#[derive(Clone)]
pub struct TransportHeartbeatHandle {
    ping: client::PingHandle,
}

impl fmt::Debug for TransportHeartbeatHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TransportHeartbeatHandle")
            .finish_non_exhaustive()
    }
}

impl TransportHeartbeatHandle {
    /// Send one SSH global request and resolve only after success/failure reply.
    pub async fn request_reply(&self) -> Result<()> {
        self.ping.send_ping().await.map_err(map_protocol_error)
    }
}

impl<V> fmt::Debug for AuthenticatedTransport<V>
where
    V: HostKeyVerifier,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthenticatedTransport")
            .finish_non_exhaustive()
    }
}

impl<V> AuthenticatedTransport<V>
where
    V: HostKeyVerifier,
{
    /// Obtain a least-privilege heartbeat capability before this transport is
    /// consumed into a Shell, exec transport, or jump stream.
    #[must_use]
    pub fn heartbeat_handle(&self) -> TransportHeartbeatHandle {
        TransportHeartbeatHandle {
            ping: self.session.ping_handle(),
        }
    }

    /// Obtain a passive close observer before consuming this transport into a
    /// resource-specific channel owner.
    #[must_use]
    pub fn close_handle(&self) -> TransportCloseHandle {
        TransportCloseHandle {
            closed: self.transport_close_observer.clone(),
        }
    }

    /// Consumes this authenticated hop and opens a bounded `direct-tcpip`
    /// channel to the next endpoint.
    ///
    /// The returned stream owns this transport as a private parent guard. When
    /// nested into `VerifiedTransport::connect_stream`, every earlier hop stays
    /// alive until the final child transport is closed or dropped. Callers
    /// cannot accidentally drop a separate parent handle while retaining only
    /// the channel stream.
    pub async fn into_direct_tcpip_stream(
        self,
        next_endpoint: Endpoint,
        open_timeout: Duration,
    ) -> Result<DirectTcpipStream> {
        if open_timeout.is_zero() {
            return Err(JumpTransportError::new(
                JumpStage::DirectTcpipOpen,
                JumpFailureKind::InvalidConfiguration,
            )
            .into());
        }
        let channel = timeout(
            open_timeout,
            self.session.channel_open_direct_tcpip(
                next_endpoint.normalized_address(),
                u32::from(next_endpoint.port()),
                "127.0.0.1",
                0,
            ),
        )
        .await
        .map_err(|_| JumpTransportError::new(JumpStage::DirectTcpipOpen, JumpFailureKind::Timeout))?
        .map_err(|error| {
            JumpTransportError::new(
                JumpStage::DirectTcpipOpen,
                if matches!(error, russh::Error::ChannelOpenFailure(_)) {
                    JumpFailureKind::Rejected
                } else {
                    JumpFailureKind::Protocol
                },
            )
        })?;
        Ok(DirectTcpipStream {
            channel: channel.into_stream(),
            _parent: Box::new(self),
        })
    }

    /// Closes an authenticated connection without opening any channel.
    pub async fn disconnect(mut self) -> Result<()> {
        let disconnect_timeout = self.timeouts.disconnect;
        complete_disconnect_within(disconnect_timeout, async move {
            self.session
                .disconnect(Disconnect::ByApplication, "", "")
                .await
                .map_err(map_protocol_error)?;
            let _completion = (&mut self.session).await;
            Ok(())
        })
        .await
    }

    /// Converts this authenticated connection into an exec-only transport.
    /// It never requests a PTY or interactive Shell and is intended for a
    /// separate capability actor such as MetricsSession.
    #[must_use]
    pub fn into_exec_transport(self) -> RemoteExecTransport<V> {
        RemoteExecTransport {
            session: self.session,
            disconnect_timeout: self.timeouts.disconnect,
            poisoned: false,
        }
    }

    /// Consumes the authenticated transport and opens one interactive shell.
    pub async fn open_remote_shell(self, initial_size: PtySize) -> Result<RemoteShell<V>> {
        let initial_size = initial_size.validate()?;
        let Self {
            session, timeouts, ..
        } = self;
        let mut channel = timeout(timeouts.channel_open, session.channel_open_session())
            .await
            .map_err(|_| TransportError::ChannelOpenTimeout)?
            .map_err(map_protocol_error)?;
        channel
            .request_pty(
                true,
                "xterm-256color",
                initial_size.columns,
                initial_size.rows,
                initial_size.pixel_width,
                initial_size.pixel_height,
                &[],
            )
            .await
            .map_err(map_protocol_error)?;
        let mut pending_events = VecDeque::new();
        await_channel_request_reply(
            &mut channel,
            timeouts.channel_request,
            ChannelRequestKind::Pty,
            &mut pending_events,
        )
        .await?;
        channel
            .request_shell(true)
            .await
            .map_err(map_protocol_error)?;
        await_channel_request_reply(
            &mut channel,
            timeouts.channel_request,
            ChannelRequestKind::Shell,
            &mut pending_events,
        )
        .await?;
        let channel_id = channel.id();
        let (read, write) = channel.split();
        Ok(RemoteShell {
            session,
            read,
            write,
            channel_id,
            pending_events,
            disconnect_timeout: timeouts.disconnect,
        })
    }

    /// Converts this authenticated connection into a port-forwarding-only
    /// transport. It does not expose PTY, Shell, exec, or authentication APIs.
    #[must_use]
    pub fn into_forward_transport(self) -> SshForwardTransport<V> {
        SshForwardTransport {
            session: self.session,
            forwarded_tcpip_rx: self.forwarded_tcpip_rx,
            remote_forward_binding: self.remote_forward_binding,
            channel_open_timeout: self.timeouts.channel_open,
            request_timeout: self.timeouts.channel_request,
            disconnect_timeout: self.timeouts.disconnect,
        }
    }

    /// Consumes this authenticated connection and opens one SFTP v3 subsystem
    /// without requesting a PTY, Shell, or exec channel.
    pub async fn open_sftp(self) -> Result<SftpTransport<V>> {
        let Self {
            session,
            timeouts,
            transport_close_observer,
            ..
        } = self;
        let mut channel = timeout(timeouts.channel_open, session.channel_open_session())
            .await
            .map_err(|_| TransportError::ChannelOpenTimeout)?
            .map_err(map_protocol_error)?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(map_protocol_error)?;
        let mut pending_events = VecDeque::new();
        await_channel_request_reply(
            &mut channel,
            timeouts.channel_request,
            ChannelRequestKind::SftpSubsystem,
            &mut pending_events,
        )
        .await?;
        let sftp = timeout(
            timeouts.channel_request,
            russh_sftp::client::SftpSession::new(channel.into_stream()),
        )
        .await
        .map_err(|_| TransportError::SftpSubsystemTimeout)?
        .map_err(|_| TransportError::SftpProtocol)?;
        Ok(SftpTransport {
            session,
            sftp,
            disconnect_timeout: timeouts.disconnect,
            transport_close_observer,
        })
    }
}

/// One authenticated transport dedicated to a single SFTP subsystem.
pub struct SftpTransport<V>
where
    V: HostKeyVerifier,
{
    session: client::Handle<ClientHandler<V>>,
    sftp: russh_sftp::client::SftpSession,
    disconnect_timeout: Duration,
    transport_close_observer: watch::Receiver<bool>,
}

impl<V> fmt::Debug for SftpTransport<V>
where
    V: HostKeyVerifier,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SftpTransport")
            .finish_non_exhaustive()
    }
}

impl<V> SftpTransport<V>
where
    V: HostKeyVerifier,
{
    #[must_use]
    pub fn client(&self) -> &russh_sftp::client::SftpSession {
        &self.sftp
    }

    #[must_use]
    pub fn close_handle(&self) -> TransportCloseHandle {
        TransportCloseHandle {
            closed: self.transport_close_observer.clone(),
        }
    }

    pub async fn disconnect(mut self) -> Result<()> {
        let disconnect_timeout = self.disconnect_timeout;
        complete_disconnect_within(disconnect_timeout, async move {
            self.sftp
                .close()
                .await
                .map_err(|_| TransportError::SftpProtocol)?;
            self.session
                .disconnect(Disconnect::ByApplication, "SFTP stopped", "")
                .await
                .map_err(map_protocol_error)?;
            let _completion = (&mut self.session).await;
            Ok(())
        })
        .await
    }
}

/// An authenticated SSH transport scoped to local, remote, or dynamic TCP
/// forwarding. The owning resource actor retains this value for its complete
/// generation so forwarded channels cannot outlive their transport.
pub struct SshForwardTransport<V>
where
    V: HostKeyVerifier,
{
    session: client::Handle<ClientHandler<V>>,
    forwarded_tcpip_rx: mpsc::Receiver<ForwardedTcpipChannel>,
    remote_forward_binding: Arc<Mutex<Option<RemoteForwardBinding>>>,
    channel_open_timeout: Duration,
    request_timeout: Duration,
    disconnect_timeout: Duration,
}

impl<V> fmt::Debug for SshForwardTransport<V>
where
    V: HostKeyVerifier,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SshForwardTransport")
            .finish_non_exhaustive()
    }
}

impl<V> SshForwardTransport<V>
where
    V: HostKeyVerifier,
{
    pub async fn open_direct_tcpip(
        &self,
        target_host: &str,
        target_port: u16,
        originator_address: &str,
        originator_port: u16,
    ) -> Result<ForwardChannel> {
        if target_host.is_empty() || originator_address.is_empty() || target_port == 0 {
            return Err(TransportError::Protocol);
        }
        let channel = timeout(
            self.channel_open_timeout,
            self.session.channel_open_direct_tcpip(
                target_host,
                u32::from(target_port),
                originator_address,
                u32::from(originator_port),
            ),
        )
        .await
        .map_err(|_| TransportError::ForwardChannelOpenTimeout)?
        .map_err(|error| {
            if matches!(error, russh::Error::ChannelOpenFailure(_)) {
                TransportError::ForwardChannelOpenRejected
            } else {
                map_protocol_error(error)
            }
        })?;
        Ok(ForwardChannel {
            stream: channel.into_stream(),
        })
    }

    pub async fn request_remote_forward(&self, bind_address: &str, bind_port: u16) -> Result<u16> {
        if bind_address.is_empty() {
            return Err(TransportError::Protocol);
        }
        {
            let mut binding = self
                .remote_forward_binding
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if binding.is_some() {
                return Err(TransportError::Protocol);
            }
            *binding = Some(RemoteForwardBinding {
                address: bind_address.to_owned(),
                port: bind_port,
                active: false,
            });
        }
        let assigned = match timeout(
            self.request_timeout,
            self.session
                .tcpip_forward(bind_address, u32::from(bind_port)),
        )
        .await
        {
            Ok(Ok(assigned)) => assigned,
            Ok(Err(error)) => {
                if matches!(error, russh::Error::RequestDenied) {
                    *self
                        .remote_forward_binding
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                    return Err(TransportError::ForwardRequestRejected);
                }
                return Err(map_protocol_error(error));
            }
            Err(_) => return Err(TransportError::ForwardRequestTimeout),
        };
        let actual_port = if bind_port == 0 {
            let assigned = u16::try_from(assigned).map_err(|_| {
                *self
                    .remote_forward_binding
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                TransportError::Protocol
            })?;
            if assigned == 0 {
                *self
                    .remote_forward_binding
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                return Err(TransportError::Protocol);
            }
            assigned
        } else {
            bind_port
        };
        *self
            .remote_forward_binding
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(RemoteForwardBinding {
            address: bind_address.to_owned(),
            port: actual_port,
            active: true,
        });
        Ok(actual_port)
    }

    pub async fn cancel_remote_forward(&self, bind_address: &str, bind_port: u16) -> Result<()> {
        {
            let mut binding = self
                .remote_forward_binding
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(active) = binding.as_mut() else {
                return Err(TransportError::Protocol);
            };
            if active.address != bind_address || active.port != bind_port {
                return Err(TransportError::Protocol);
            }
            active.active = false;
        }
        timeout(
            self.request_timeout,
            self.session
                .cancel_tcpip_forward(bind_address, u32::from(bind_port)),
        )
        .await
        .map_err(|_| TransportError::ForwardRequestTimeout)?
        .map_err(|error| {
            if matches!(error, russh::Error::RequestDenied) {
                TransportError::ForwardRequestRejected
            } else {
                map_protocol_error(error)
            }
        })?;
        *self
            .remote_forward_binding
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        Ok(())
    }

    pub async fn next_forwarded_tcpip(&mut self) -> Option<ForwardedTcpipChannel> {
        self.forwarded_tcpip_rx.recv().await
    }

    pub async fn disconnect(self) -> Result<()> {
        if let Some(binding) = self
            .remote_forward_binding
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
        {
            binding.active = false;
        }
        timeout(
            self.disconnect_timeout,
            self.session
                .disconnect(Disconnect::ByApplication, "forward stopped", ""),
        )
        .await
        .map_err(|_| TransportError::DisconnectTimeout)?
        .map_err(map_protocol_error)
    }
}

/// One SSH channel opened by a local/dynamic forward operation.
pub struct ForwardChannel {
    stream: russh::ChannelStream<client::Msg>,
}

impl fmt::Debug for ForwardChannel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ForwardChannel")
            .finish_non_exhaustive()
    }
}

impl AsyncRead for ForwardChannel {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(context, buffer)
    }
}

impl AsyncWrite for ForwardChannel {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(context, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(context)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(context)
    }
}

/// One connection accepted by a server-side remote-forward listener.
pub struct ForwardedTcpipChannel {
    pub connected_address: String,
    pub connected_port: u16,
    pub originator_address: String,
    pub originator_port: u16,
    stream: russh::ChannelStream<client::Msg>,
}

impl fmt::Debug for ForwardedTcpipChannel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ForwardedTcpipChannel")
            .field("connected_address", &self.connected_address)
            .field("connected_port", &self.connected_port)
            .field("originator_address", &self.originator_address)
            .field("originator_port", &self.originator_port)
            .finish_non_exhaustive()
    }
}

impl AsyncRead for ForwardedTcpipChannel {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(context, buffer)
    }
}

impl AsyncWrite for ForwardedTcpipChannel {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(context, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(context)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(context)
    }
}

/// A jump channel whose parent authenticated SSH transport is owned for the
/// complete lifetime of the stream.
#[must_use = "dropping this stream closes the jump channel and releases its parent transport"]
pub struct DirectTcpipStream {
    channel: russh::ChannelStream<client::Msg>,
    _parent: Box<dyn Send + 'static>,
}

impl fmt::Debug for DirectTcpipStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DirectTcpipStream")
            .finish_non_exhaustive()
    }
}

impl AsyncRead for DirectTcpipStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.channel).poll_read(context, buffer)
    }
}

impl AsyncWrite for DirectTcpipStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.channel).poll_write(context, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.channel).poll_flush(context)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.channel).poll_shutdown(context)
    }
}

/// An authenticated SSH transport dedicated to bounded non-interactive exec
/// channels. It must not be shared with Terminal, SFTP, or forwarding actors.
pub struct RemoteExecTransport<V>
where
    V: HostKeyVerifier,
{
    session: client::Handle<ClientHandler<V>>,
    disconnect_timeout: Duration,
    poisoned: bool,
}

impl<V> fmt::Debug for RemoteExecTransport<V>
where
    V: HostKeyVerifier,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteExecTransport")
            .finish_non_exhaustive()
    }
}

impl<V> RemoteExecTransport<V>
where
    V: HostKeyVerifier,
{
    pub async fn execute_capture(
        &mut self,
        command: &'static [u8],
        command_timeout: Duration,
        max_output_bytes: usize,
    ) -> Result<RemoteExecOutput> {
        if command.len() > 4 * 1024 {
            return Err(TransportError::InvalidRemoteExecCommand);
        }
        self.execute_capture_request(command, None, command_timeout, max_output_bytes, || true)
            .await
    }

    /// Executes a separately authorized, bounded request on this dedicated
    /// transport. The final admission callback runs after channel creation and
    /// immediately before dispatch; callers retain cancellation and ownership.
    /// This does not make arbitrary commands safe for unapproved callers.
    async fn execute_capture_request(
        &mut self,
        command: &[u8],
        stdin: Option<&[u8]>,
        command_timeout: Duration,
        max_output_bytes: usize,
        before_dispatch: impl FnOnce() -> bool,
    ) -> Result<RemoteExecOutput> {
        if self.poisoned {
            return Err(TransportError::ConnectionLost);
        }
        if command.is_empty()
            || command.len() > 64 * 1024
            || command.contains(&0)
            || stdin.is_some_and(|bytes| bytes.len() > 256 * 1024)
            || command_timeout.is_zero()
            || max_output_bytes == 0
        {
            return Err(TransportError::InvalidRemoteExecCommand);
        }
        let capture = async {
            let channel = self
                .session
                .channel_open_session()
                .await
                .map_err(map_protocol_error)?;
            if !before_dispatch() {
                let _ = channel.close().await;
                return Err(TransportError::RemoteExecRejected);
            }
            channel
                .exec(true, command)
                .await
                .map_err(map_protocol_error)?;
            let (mut reader, writer) = channel.split();
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let mut exit_status = None;
            let read_output = async {
                while let Some(message) = reader.wait().await {
                    match message {
                        ChannelMsg::Data { data } => {
                            extend_bounded(&mut stdout, &stderr, &data, max_output_bytes)?;
                        }
                        ChannelMsg::ExtendedData { data, .. } => {
                            extend_bounded(&mut stderr, &stdout, &data, max_output_bytes)?;
                        }
                        ChannelMsg::ExitStatus {
                            exit_status: status,
                        } => {
                            exit_status = Some(status);
                        }
                        ChannelMsg::Failure => return Err(TransportError::RemoteExecRejected),
                        _ => {}
                    }
                }
                Ok(RemoteExecOutput {
                    stdout,
                    stderr,
                    exit_status,
                })
            };
            if let Some(input) = stdin {
                let send_input = async {
                    writer.data(input).await.map_err(map_protocol_error)?;
                    writer.eof().await.map_err(map_protocol_error)
                };
                let (_, output) = tokio::try_join!(send_input, read_output)?;
                Ok(output)
            } else {
                read_output.await
            }
        };
        match timeout(command_timeout, capture).await {
            Err(_) => {
                self.poison().await;
                Err(TransportError::RemoteExecTimeout)
            }
            Ok(Err(error @ TransportError::RemoteExecOutputTooLarge))
            | Ok(Err(error @ TransportError::ConnectionLost))
            | Ok(Err(error @ TransportError::Protocol)) => {
                self.poison().await;
                Err(error)
            }
            Ok(result) => result,
        }
    }

    pub async fn disconnect(mut self) -> Result<()> {
        if self.poisoned {
            return Ok(());
        }
        let disconnect_timeout = self.disconnect_timeout;
        complete_disconnect_within(disconnect_timeout, async move {
            self.session
                .disconnect(Disconnect::ByApplication, "", "")
                .await
                .map_err(map_protocol_error)?;
            let _completion = (&mut self.session).await;
            Ok(())
        })
        .await
    }

    async fn poison(&mut self) {
        self.poisoned = true;
        if self
            .session
            .disconnect(Disconnect::ByApplication, "", "")
            .await
            .is_ok()
        {
            let _ = timeout(self.disconnect_timeout, &mut self.session).await;
        }
    }
}

fn extend_bounded(
    target: &mut Vec<u8>,
    other: &[u8],
    data: &[u8],
    max_output_bytes: usize,
) -> Result<()> {
    if target
        .len()
        .saturating_add(other.len())
        .saturating_add(data.len())
        > max_output_bytes
    {
        return Err(TransportError::RemoteExecOutputTooLarge);
    }
    target.extend_from_slice(data);
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteExecOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_status: Option<u32>,
}

impl<V> client::Handler for ClientHandler<V>
where
    V: HostKeyVerifier,
{
    type Error = TransportError;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool> {
        let public_key = server_public_key.public_key();
        let public_key_blob = public_key.public_key_bytes();
        if public_key_blob.is_empty() {
            return Err(TransportError::HostKeyVerificationFailed);
        }
        let observed = ObservedHostKey {
            algorithm: public_key.algorithm().to_string(),
            fingerprint_sha256: ssh_sha256_fingerprint(&public_key_blob),
            public_key_blob,
        };
        let decision = timeout(
            self.host_key_decision_timeout,
            self.verifier
                .verify(self.endpoint.clone(), observed.clone()),
        )
        .await
        .map_err(|_| TransportError::HostKeyDecisionTimeout)??;
        match decision {
            HostKeyDecision::Trusted => Ok(true),
            HostKeyDecision::Rejected => Err(TransportError::HostKeyRejected),
            HostKeyDecision::Mismatch {
                trusted_fingerprint,
            } => Err(TransportError::HostKeyMismatch {
                trusted_fingerprint,
                observed_fingerprint: observed.fingerprint_sha256,
            }),
        }
    }

    async fn kex_done(
        &mut self,
        _shared_secret: Option<&[u8]>,
        names: &russh::Names,
        _session: &mut client::Session,
    ) -> Result<()> {
        let negotiated = NegotiatedAlgorithms {
            key_exchange: names.kex.as_ref().to_owned(),
            host_key: names.key.to_string(),
            cipher_client_to_server: names.client_cipher.as_ref().to_owned(),
            cipher_server_to_client: names.server_cipher.as_ref().to_owned(),
            mac_client_to_server: names.client_mac.as_ref().to_owned(),
            mac_server_to_client: names.server_mac.as_ref().to_owned(),
        };
        *self
            .negotiated_algorithms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(negotiated);
        Ok(())
    }

    fn disconnected(
        &mut self,
        reason: client::DisconnectReason<Self::Error>,
    ) -> impl Future<Output = Result<()>> + Send {
        self.transport_closed.send_replace(true);
        async move {
            match reason {
                client::DisconnectReason::ReceivedDisconnect(_) => Ok(()),
                client::DisconnectReason::Error(error) => Err(error),
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<client::Msg>,
        connected_address: &str,
        connected_port: u32,
        originator_address: &str,
        originator_port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> impl Future<Output = Result<()>> + Send {
        let shared_sender = self
            .shared_remote_forwards
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sender_for(
                connected_address,
                connected_port,
                originator_address,
                originator_port,
            );
        let binding = self
            .remote_forward_binding
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let forwarded_tcpip_tx = self.forwarded_tcpip_tx.clone();
        let connected_address = connected_address.to_owned();
        let originator_address = originator_address.to_owned();
        async move {
            if let Some(sender) = shared_sender {
                let Ok(permit) = sender.try_reserve_owned() else {
                    reply.reject(ChannelOpenFailure::ResourceShortage).await;
                    return Ok(());
                };
                reply.accept().await;
                permit.send(ForwardedTcpipChannel {
                    connected_address,
                    connected_port: connected_port as u16,
                    originator_address,
                    originator_port: originator_port as u16,
                    stream: channel.into_stream(),
                });
                return Ok(());
            }

            let Some((connected_port, originator_port)) = validated_forwarded_tcpip_metadata(
                binding.as_ref(),
                &connected_address,
                connected_port,
                &originator_address,
                originator_port,
            ) else {
                reply
                    .reject(ChannelOpenFailure::AdministrativelyProhibited)
                    .await;
                return Ok(());
            };
            let Ok(permit) = forwarded_tcpip_tx.try_reserve_owned() else {
                reply.reject(ChannelOpenFailure::ResourceShortage).await;
                return Ok(());
            };
            reply.accept().await;
            permit.send(ForwardedTcpipChannel {
                connected_address,
                connected_port,
                originator_address,
                originator_port,
                stream: channel.into_stream(),
            });
            Ok(())
        }
    }
}

/// A single authenticated SSH transport with one interactive Shell Channel.
///
/// The read and write halves are separated so a session actor can own them in
/// independent bounded tasks without a mutex held across network awaits.
pub struct RemoteShell<V>
where
    V: HostKeyVerifier,
{
    session: client::Handle<ClientHandler<V>>,
    read: ChannelReadHalf,
    write: ChannelWriteHalf<client::Msg>,
    channel_id: ChannelId,
    pending_events: VecDeque<ChannelMsg>,
    disconnect_timeout: Duration,
}

impl<V> fmt::Debug for RemoteShell<V>
where
    V: HostKeyVerifier,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteShell")
            .field("channel_id", &self.channel_id)
            .finish_non_exhaustive()
    }
}

impl<V> RemoteShell<V>
where
    V: HostKeyVerifier,
{
    #[must_use]
    pub const fn channel_id(&self) -> ChannelId {
        self.channel_id
    }

    pub async fn send_input(&self, data: impl Into<Bytes>) -> Result<()> {
        self.write
            .data_bytes(data)
            .await
            .map_err(map_protocol_error)
    }

    pub async fn resize(&self, size: PtySize) -> Result<()> {
        if size.columns == 0 || size.rows == 0 {
            return Err(TransportError::InvalidPtySize);
        }
        self.write
            .window_change(size.columns, size.rows, size.pixel_width, size.pixel_height)
            .await
            .map_err(map_protocol_error)
    }

    pub async fn next_event(&mut self) -> Result<ShellEvent> {
        if let Some(message) = self.pending_events.pop_front() {
            return Ok(ShellEvent::from(message));
        }
        tokio::select! {
            result = &mut self.session => {
                let _ = result;
                Err(TransportError::ConnectionLost)
            }
            message = self.read.wait() => {
                message.map(ShellEvent::from).ok_or(TransportError::ConnectionLost)
            }
        }
    }

    pub async fn disconnect(mut self) -> Result<()> {
        let disconnect_timeout = self.disconnect_timeout;
        complete_disconnect_within(disconnect_timeout, async move {
            let _ = self.write.eof().await;
            let _ = self.write.close().await;
            self.session
                .disconnect(Disconnect::ByApplication, "", "")
                .await
                .map_err(map_protocol_error)?;
            let _completion = (&mut self.session).await;
            // Russh resolves its Handle with Error::Disconnect after a locally
            // requested disconnect, even when the disconnect packet was queued
            // and the session task shut down normally. Reaching completion is
            // the success fact; the total deadline also bounds every enqueue
            // before this point so the owning actor can always converge.
            Ok(())
        })
        .await
    }
}

async fn complete_disconnect_within<F>(disconnect_timeout: Duration, disconnect: F) -> Result<()>
where
    F: Future<Output = Result<()>>,
{
    timeout(disconnect_timeout, disconnect)
        .await
        .map_err(|_| TransportError::DisconnectTimeout)?
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellEvent {
    Data(Bytes),
    ExtendedData {
        data: Bytes,
        stream: u32,
    },
    ExitStatus(u32),
    ExitSignal {
        signal: String,
        core_dumped: bool,
        message: String,
    },
    Eof,
    Closed,
    RequestSucceeded,
    RequestFailed,
    Other,
}

impl From<ChannelMsg> for ShellEvent {
    fn from(message: ChannelMsg) -> Self {
        match message {
            ChannelMsg::Data { data } => Self::Data(data),
            ChannelMsg::ExtendedData { data, ext } => Self::ExtendedData { data, stream: ext },
            ChannelMsg::ExitStatus { exit_status } => Self::ExitStatus(exit_status),
            ChannelMsg::ExitSignal {
                signal_name,
                core_dumped,
                error_message,
                ..
            } => Self::ExitSignal {
                signal: format!("{signal_name:?}"),
                core_dumped,
                message: error_message,
            },
            ChannelMsg::Eof => Self::Eof,
            ChannelMsg::Close => Self::Closed,
            ChannelMsg::Success => Self::RequestSucceeded,
            ChannelMsg::Failure => Self::RequestFailed,
            _ => Self::Other,
        }
    }
}

#[derive(Clone, Copy)]
enum ChannelRequestKind {
    Pty,
    Shell,
    Exec,
    SftpSubsystem,
}

async fn await_channel_request_reply(
    channel: &mut Channel<client::Msg>,
    request_timeout: Duration,
    kind: ChannelRequestKind,
    pending_events: &mut VecDeque<ChannelMsg>,
) -> Result<()> {
    loop {
        let message = timeout(request_timeout, channel.wait())
            .await
            .map_err(|_| match kind {
                ChannelRequestKind::Pty => TransportError::PtyRequestTimeout,
                ChannelRequestKind::Shell => TransportError::ShellRequestTimeout,
                ChannelRequestKind::Exec => TransportError::RemoteExecTimeout,
                ChannelRequestKind::SftpSubsystem => TransportError::SftpSubsystemTimeout,
            })?
            .ok_or(TransportError::ConnectionLost)?;
        match message {
            ChannelMsg::Success => return Ok(()),
            ChannelMsg::Failure => {
                return Err(match kind {
                    ChannelRequestKind::Pty => TransportError::PtyRejected,
                    ChannelRequestKind::Shell => TransportError::ShellRejected,
                    ChannelRequestKind::Exec => TransportError::RemoteExecRejected,
                    ChannelRequestKind::SftpSubsystem => TransportError::SftpSubsystemRejected,
                });
            }
            other => pending_events.push_back(other),
        }
    }
}

fn client_config(algorithm_policy: &AlgorithmPolicy) -> client::Config {
    client::Config {
        // Host heartbeat defaults to Disabled. A later M2 transport actor owns
        // explicit policy scheduling and observable reply/timeout facts.
        keepalive_interval: None,
        keepalive_max: 0,
        nodelay: true,
        preferred: algorithm_policy.preferred(),
        ..client::Config::default()
    }
}

async fn authenticate<V>(
    session: &mut client::Handle<ClientHandler<V>>,
    username: String,
    authentication: Authentication,
) -> Result<()>
where
    V: HostKeyVerifier,
{
    let outcome = match authentication {
        Authentication::Password { password } => {
            let password = String::from_utf8(password.to_vec())
                .map(Zeroizing::new)
                .map_err(|_| TransportError::AuthenticationRejected)?;
            // Russh 0.63.1 necessarily creates an internal String copy here.
            // The caller and this boundary still zero their scoped copies.
            session
                .authenticate_password(username, password.as_str())
                .await
                .map_err(map_protocol_error)?
        }
        Authentication::PrivateKey {
            private_key,
            passphrase,
        } => {
            if private_key.is_empty() || private_key.len() > MAX_PRIVATE_KEY_BYTES {
                return Err(TransportError::InvalidPrivateKey);
            }
            let private_key = std::str::from_utf8(private_key.as_slice())
                .map_err(|_| TransportError::InvalidPrivateKey)?;
            let passphrase = passphrase
                .as_ref()
                .map(|value| std::str::from_utf8(value.as_slice()))
                .transpose()
                .map_err(|_| TransportError::InvalidPrivateKey)?;
            let key = decode_secret_key(private_key, passphrase)
                .map_err(|_| TransportError::InvalidPrivateKey)?;
            let rsa_hash = if key.algorithm().is_rsa() {
                match session
                    .best_supported_rsa_hash()
                    .await
                    .map_err(map_protocol_error)?
                {
                    Some(Some(HashAlg::Sha512)) => Some(HashAlg::Sha512),
                    Some(Some(HashAlg::Sha256)) => Some(HashAlg::Sha256),
                    Some(Some(_)) => return Err(TransportError::InsecureRsaSignatureOnly),
                    Some(None) => return Err(TransportError::InsecureRsaSignatureOnly),
                    None => Some(HashAlg::Sha512),
                }
            } else {
                None
            };
            session
                .authenticate_publickey(
                    username,
                    PrivateKeyWithHashAlg::new(Arc::new(key), rsa_hash),
                )
                .await
                .map_err(map_protocol_error)?
        }
        Authentication::SshAgent {
            identity,
            mut client,
        } => {
            let public_key = identity.public_key().into_owned();
            let rsa_hash = if public_key.algorithm().is_rsa() {
                match session
                    .best_supported_rsa_hash()
                    .await
                    .map_err(map_protocol_error)?
                {
                    Some(Some(HashAlg::Sha512)) => Some(HashAlg::Sha512),
                    Some(Some(HashAlg::Sha256)) => Some(HashAlg::Sha256),
                    Some(Some(_)) | Some(None) => {
                        return Err(TransportError::InsecureRsaSignatureOnly);
                    }
                    None => Some(HashAlg::Sha512),
                }
            } else {
                None
            };
            session
                .authenticate_publickey_with(username, public_key, rsa_hash, &mut client)
                .await
                .map_err(|_| TransportError::SshAgentUnavailable)?
        }
        Authentication::SshAgentCertificate {
            certificate,
            mut client,
        } => {
            let public_key = certificate.public_key();
            let rsa_hash = if public_key.algorithm().is_rsa() {
                match session
                    .best_supported_rsa_hash()
                    .await
                    .map_err(map_protocol_error)?
                {
                    Some(Some(HashAlg::Sha512)) => Some(HashAlg::Sha512),
                    Some(Some(HashAlg::Sha256)) => Some(HashAlg::Sha256),
                    Some(Some(_)) | Some(None) => {
                        return Err(TransportError::InsecureRsaSignatureOnly);
                    }
                    None => Some(HashAlg::Sha512),
                }
            } else {
                None
            };
            session
                .authenticate_certificate_with(username, *certificate, rsa_hash, &mut client)
                .await
                .map_err(|_| TransportError::SshAgentUnavailable)?
        }
    };
    authentication_result(outcome)
}

fn authentication_result(outcome: client::AuthResult) -> Result<()> {
    match outcome {
        client::AuthResult::Success => Ok(()),
        client::AuthResult::Failure {
            partial_success, ..
        } => authentication_failure(partial_success),
    }
}

fn authentication_failure(partial_success: bool) -> Result<()> {
    if partial_success {
        Err(TransportError::AuthenticationIncomplete)
    } else {
        Err(TransportError::AuthenticationRejected)
    }
}

fn validate_username(username: &str) -> Result<()> {
    if username.is_empty()
        || username.len() > MAX_USERNAME_BYTES
        || username.chars().any(char::is_control)
    {
        return Err(TransportError::InvalidUsername);
    }
    Ok(())
}

fn validated_keyboard_interactive_challenge(
    name: String,
    instructions: String,
    prompts: Vec<client::Prompt>,
) -> Result<KeyboardInteractiveChallenge> {
    let total_bytes = name
        .len()
        .saturating_add(instructions.len())
        .saturating_add(
            prompts
                .iter()
                .map(|prompt| prompt.prompt.len())
                .sum::<usize>(),
        );
    if name.len() > MAX_KEYBOARD_INTERACTIVE_TEXT_BYTES
        || instructions.len() > MAX_KEYBOARD_INTERACTIVE_TEXT_BYTES
        || prompts.len() > MAX_KEYBOARD_INTERACTIVE_PROMPTS_PER_ROUND
        || prompts
            .iter()
            .any(|prompt| prompt.prompt.len() > MAX_KEYBOARD_INTERACTIVE_TEXT_BYTES)
        || total_bytes > MAX_KEYBOARD_INTERACTIVE_ROUND_BYTES
    {
        return Err(TransportError::InvalidKeyboardInteractiveResponse);
    }
    Ok(KeyboardInteractiveChallenge {
        name: escaped_untrusted_prompt_text(&name),
        instructions: escaped_untrusted_prompt_text(&instructions),
        prompts: prompts
            .into_iter()
            .map(|prompt| KeyboardInteractivePrompt {
                text: escaped_untrusted_prompt_text(&prompt.prompt),
                echo: prompt.echo,
            })
            .collect(),
    })
}

fn escaped_untrusted_prompt_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_control() && !matches!(character, '\n' | '\t') {
            use std::fmt::Write as _;
            let _ = write!(escaped, "\\u{{{:x}}}", u32::from(character));
        } else {
            escaped.push(character);
        }
    }
    escaped
}

fn map_protocol_error(error: impl fmt::Debug) -> TransportError {
    let _ = error;
    TransportError::Protocol
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        future::pending,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use bytes::Bytes;
    use norishell_ssh_domain::{Endpoint, ssh_sha256_fingerprint};
    use russh::{
        Channel, ChannelId, ChannelOpenFailure, Pty,
        keys::{Algorithm, PrivateKey, PublicKeyBase64, ssh_key::LineEnding},
        server::{self, Auth, Msg, Session},
    };
    use russh_sftp::protocol::{Packet as SftpPacket, Version as SftpVersion};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt, copy_bidirectional},
        net::{TcpListener, TcpStream},
        sync::{oneshot, watch},
        task::JoinHandle,
        time::timeout,
    };

    use super::{
        AgentClient, AgentIdentity, AlgorithmCategory, AlgorithmPolicy, Authentication,
        ConnectRequest, HostKeyDecision, HostKeyVerifier, IngressFailureKind, IngressStage,
        JumpFailureKind, JumpStage, ObservedHostKey, ProxyCredentials, PtySize,
        RemoteExecStreamEvent, RemoteExecStreamLimits, RemoteForwardBinding, RemoteShell,
        RouteIngress, ShellEvent, TransportError, VerifiedTransport, VerifyFuture,
        algorithm_policy_catalog, client_config, inspect_private_key,
        validated_forwarded_tcpip_metadata, validated_keyboard_interactive_challenge,
    };

    const TEST_USER: &str = "tester";
    const TEST_PASSWORD: &str = "correct horse battery staple";

    #[test]
    fn transport_does_not_enable_implicit_keepalive_for_disabled_hosts() {
        let config = client_config(&AlgorithmPolicy::secure_default());
        assert_eq!(config.keepalive_interval, None);
        assert_eq!(config.keepalive_max, 0);
    }

    #[test]
    fn forwarded_tcpip_requires_an_exact_active_remote_binding() {
        let active = RemoteForwardBinding {
            address: "127.0.0.1".to_owned(),
            port: 4022,
            active: true,
        };
        assert_eq!(
            validated_forwarded_tcpip_metadata(
                Some(&active),
                "127.0.0.1",
                4022,
                "198.51.100.25",
                51000,
            ),
            Some((4022, 51000))
        );
        assert_eq!(
            validated_forwarded_tcpip_metadata(None, "127.0.0.1", 4022, "198.51.100.25", 51000,),
            None
        );
        assert_eq!(
            validated_forwarded_tcpip_metadata(
                Some(&RemoteForwardBinding {
                    active: false,
                    ..active.clone()
                }),
                "127.0.0.1",
                4022,
                "198.51.100.25",
                51000,
            ),
            None
        );
        assert_eq!(
            validated_forwarded_tcpip_metadata(
                Some(&active),
                "127.0.0.1",
                4023,
                "198.51.100.25",
                51000,
            ),
            None
        );
        assert_eq!(
            validated_forwarded_tcpip_metadata(
                Some(&active),
                "127.0.0.1",
                4022,
                "198.51.100.25",
                u32::from(u16::MAX) + 1,
            ),
            None
        );
    }

    #[test]
    fn no_common_algorithm_keeps_only_bounded_safe_candidate_names() {
        let error = TransportError::from(russh::Error::NoCommonAlgo {
            kind: russh::AlgorithmKind::Cipher,
            ours: vec!["aes256-ctr".to_owned(), "aes256-ctr".to_owned()],
            theirs: vec![
                "legacy-cipher@example.test".to_owned(),
                "bad\nname".to_owned(),
                "x".repeat(81),
            ],
        });
        assert!(matches!(
            error,
            TransportError::AlgorithmNegotiationFailed {
                category: AlgorithmCategory::Cipher,
                ref client_candidates,
                ref server_candidates,
            } if client_candidates == &["aes256-ctr"]
                && server_candidates == &["legacy-cipher@example.test"]
        ));
    }

    #[test]
    fn controlled_compatibility_exceptions_append_only_catalog_algorithms() {
        let policy = AlgorithmPolicy::from_catalog_ids(
            "secure-default",
            [
                (AlgorithmCategory::KeyExchange, "compat-kex-dh-group14-sha1"),
                (AlgorithmCategory::HostKey, "compat-host-key-rsa-sha1"),
                (AlgorithmCategory::Cipher, "compat-cipher-aes256-cbc"),
                (AlgorithmCategory::Mac, "compat-mac-hmac-sha1"),
            ],
        )
        .expect("catalog exception policy");
        let preferred = client_config(&policy).preferred;
        assert!(preferred.kex.contains(&russh::kex::DH_G14_SHA1));
        assert_eq!(preferred.key.last(), Some(&Algorithm::Rsa { hash: None }));
        assert_eq!(preferred.cipher.last(), Some(&russh::cipher::AES_256_CBC));
        assert_eq!(preferred.mac.last(), Some(&russh::mac::HMAC_SHA1));
        assert!(!preferred.cipher.contains(&russh::cipher::NONE));
        assert!(!preferred.mac.contains(&russh::mac::NONE));
    }

    #[test]
    fn algorithm_policy_rejects_unknown_mismatched_and_duplicate_exceptions() {
        assert!(
            AlgorithmPolicy::from_catalog_ids(
                "custom",
                std::iter::empty::<(AlgorithmCategory, &str)>(),
            )
            .is_err()
        );
        assert!(
            AlgorithmPolicy::from_catalog_ids(
                "secure-default",
                [(AlgorithmCategory::Cipher, "compat-kex-dh-group14-sha1")],
            )
            .is_err()
        );
        assert!(
            AlgorithmPolicy::from_catalog_ids(
                "secure-default",
                [
                    (AlgorithmCategory::Mac, "compat-mac-hmac-sha1"),
                    (AlgorithmCategory::Mac, "compat-mac-hmac-sha1"),
                ],
            )
            .is_err()
        );
        assert!(
            algorithm_policy_catalog()
                .iter()
                .all(|entry| entry.algorithm_name != "none")
        );
    }

    #[test]
    fn keyboard_interactive_challenge_is_bounded_and_escapes_server_controls() {
        let challenge = validated_keyboard_interactive_challenge(
            "name\u{1b}".to_owned(),
            "line one\nline two".to_owned(),
            vec![russh::client::Prompt {
                prompt: "code\u{7}".to_owned(),
                echo: false,
            }],
        )
        .expect("bounded challenge");
        assert_eq!(challenge.name, "name\\u{1b}");
        assert_eq!(challenge.instructions, "line one\nline two");
        assert_eq!(challenge.prompts[0].text, "code\\u{7}");
        assert!(!challenge.prompts[0].echo);

        let oversized = validated_keyboard_interactive_challenge(
            "name".to_owned(),
            String::new(),
            vec![russh::client::Prompt {
                prompt: "x".repeat(4 * 1024 + 1),
                echo: true,
            }],
        )
        .expect_err("oversized prompt must fail closed");
        assert!(matches!(
            oversized,
            TransportError::InvalidKeyboardInteractiveResponse
        ));
    }

    #[derive(Clone)]
    struct RecordingVerifier {
        decision: HostKeyDecision,
        observations: Arc<Mutex<Vec<(Endpoint, ObservedHostKey)>>>,
    }

    impl RecordingVerifier {
        fn new(decision: HostKeyDecision) -> Self {
            Self {
                decision,
                observations: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn observations(&self) -> Vec<(Endpoint, ObservedHostKey)> {
            self.observations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl HostKeyVerifier for RecordingVerifier {
        fn verify(&self, endpoint: Endpoint, observed: ObservedHostKey) -> VerifyFuture {
            let observations = Arc::clone(&self.observations);
            let decision = self.decision.clone();
            Box::pin(async move {
                observations
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push((endpoint, observed));
                Ok(decision)
            })
        }
    }

    #[derive(Default)]
    struct ServerEvidence {
        tcp_connections: AtomicUsize,
        auth_attempts: AtomicUsize,
        shell_requests: AtomicUsize,
        exec_requests: Mutex<Vec<Vec<u8>>>,
        pty_requests: Mutex<Vec<(String, u32, u32)>>,
        resizes: Mutex<Vec<(u32, u32)>>,
        terminal_channels: Mutex<HashSet<ChannelId>>,
        sftp_channels: Mutex<HashSet<ChannelId>>,
        exec_stdin_channels: Mutex<HashSet<ChannelId>>,
        closed_channels: Mutex<HashSet<ChannelId>>,
    }

    #[derive(Clone)]
    struct TestServerHandler {
        evidence: Arc<ServerEvidence>,
        accepted_public_key: Option<Vec<u8>>,
        reject_pty: bool,
        direct_tcpip_behavior: DirectTcpipBehavior,
        keyboard_interactive_round: u8,
    }

    #[derive(Clone, Copy)]
    enum DirectTcpipBehavior {
        Forward,
        Reject,
        Stall,
    }

    impl server::Handler for TestServerHandler {
        type Error = russh::Error;

        async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
            self.evidence.auth_attempts.fetch_add(1, Ordering::SeqCst);
            Ok(if user == TEST_USER && password == TEST_PASSWORD {
                Auth::Accept
            } else {
                Auth::reject()
            })
        }

        async fn auth_keyboard_interactive<'a>(
            &'a mut self,
            user: &str,
            _submethods: &str,
            response: Option<server::Response<'a>>,
        ) -> Result<Auth, Self::Error> {
            use std::borrow::Cow;

            self.evidence.auth_attempts.fetch_add(1, Ordering::SeqCst);
            if user != TEST_USER {
                return Ok(Auth::reject());
            }
            match (self.keyboard_interactive_round, response) {
                (0, None) => {
                    self.keyboard_interactive_round = 1;
                    Ok(Auth::Partial {
                        name: Cow::Borrowed("Test verification"),
                        instructions: Cow::Borrowed("Complete both bounded rounds"),
                        prompts: Cow::Borrowed(&[
                            (Cow::Borrowed("Account"), true),
                            (Cow::Borrowed("Code"), false),
                        ]),
                    })
                }
                (1, Some(response)) => {
                    let values = response
                        .map(|value| String::from_utf8_lossy(&value).into_owned())
                        .collect::<Vec<_>>();
                    if values == [TEST_USER, "111111"] {
                        self.keyboard_interactive_round = 2;
                        Ok(Auth::Partial {
                            name: Cow::Borrowed("Confirmation"),
                            instructions: Cow::Borrowed("Confirm this test connection"),
                            prompts: Cow::Borrowed(&[(Cow::Borrowed("Continue"), true)]),
                        })
                    } else {
                        Ok(Auth::reject())
                    }
                }
                (2, Some(response)) => {
                    let values = response
                        .map(|value| String::from_utf8_lossy(&value).into_owned())
                        .collect::<Vec<_>>();
                    if values == ["yes"] {
                        Ok(Auth::Accept)
                    } else {
                        Ok(Auth::reject())
                    }
                }
                _ => Ok(Auth::reject()),
            }
        }

        async fn auth_publickey(
            &mut self,
            user: &str,
            public_key: &russh::keys::PublicKey,
        ) -> Result<Auth, Self::Error> {
            self.evidence.auth_attempts.fetch_add(1, Ordering::SeqCst);
            Ok(
                if user == TEST_USER
                    && self
                        .accepted_public_key
                        .as_ref()
                        .is_some_and(|accepted| accepted == &public_key.public_key_bytes())
                {
                    Auth::Accept
                } else {
                    Auth::reject()
                },
            )
        }

        async fn channel_open_session(
            &mut self,
            channel: Channel<Msg>,
            reply: server::ChannelOpenHandle,
            _session: &mut Session,
        ) -> Result<(), Self::Error> {
            self.evidence
                .terminal_channels
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(channel.id());
            reply.accept().await;
            Ok(())
        }

        async fn channel_open_direct_tcpip(
            &mut self,
            channel: Channel<Msg>,
            host_to_connect: &str,
            port_to_connect: u32,
            _originator_address: &str,
            _originator_port: u32,
            reply: server::ChannelOpenHandle,
            _session: &mut Session,
        ) -> Result<(), Self::Error> {
            match self.direct_tcpip_behavior {
                DirectTcpipBehavior::Forward => {
                    let Ok(port) = u16::try_from(port_to_connect) else {
                        reply.reject(ChannelOpenFailure::ConnectFailed).await;
                        return Ok(());
                    };
                    let Ok(mut upstream) = TcpStream::connect((host_to_connect, port)).await else {
                        reply.reject(ChannelOpenFailure::ConnectFailed).await;
                        return Ok(());
                    };
                    reply.accept().await;
                    tokio::spawn(async move {
                        let mut channel = channel.into_stream();
                        let _ = copy_bidirectional(&mut channel, &mut upstream).await;
                    });
                }
                DirectTcpipBehavior::Reject => {
                    reply
                        .reject(ChannelOpenFailure::AdministrativelyProhibited)
                        .await;
                }
                DirectTcpipBehavior::Stall => pending::<()>().await,
            }
            Ok(())
        }

        async fn pty_request(
            &mut self,
            channel: ChannelId,
            term: &str,
            columns: u32,
            rows: u32,
            _pixel_width: u32,
            _pixel_height: u32,
            _modes: &[(Pty, u32)],
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            self.evidence
                .pty_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((term.to_owned(), columns, rows));
            if self.reject_pty {
                session.channel_failure(channel)?;
            } else {
                session.channel_success(channel)?;
            }
            Ok(())
        }

        async fn shell_request(
            &mut self,
            channel: ChannelId,
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            self.evidence.shell_requests.fetch_add(1, Ordering::SeqCst);
            session.channel_success(channel)?;
            session.data(
                channel,
                b"\x1b[31m\xe6\xac\xa2\xe8\xbf\x8e\x1b[0m\r\n".as_slice(),
            )?;
            Ok(())
        }

        async fn exec_request(
            &mut self,
            channel: ChannelId,
            data: &[u8],
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            self.evidence
                .exec_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(data.to_vec());
            session.channel_success(channel)?;
            if data == b"stdin-echo" {
                self.evidence
                    .exec_stdin_channels
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(channel);
                return Ok(());
            }
            if data == b"stall-exec" {
                return Ok(());
            }
            session.data(channel, b"metric-out\n".as_slice())?;
            session.extended_data(channel, 1, b"metric-warning\n".as_slice())?;
            session.exit_status_request(channel, 0)?;
            session.eof(channel)?;
            session.close(channel)?;
            Ok(())
        }

        async fn subsystem_request(
            &mut self,
            channel: ChannelId,
            name: &str,
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            if name == "sftp" {
                self.evidence
                    .terminal_channels
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&channel);
                self.evidence
                    .sftp_channels
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(channel);
                session.channel_success(channel)?;
            } else {
                session.channel_failure(channel)?;
            }
            Ok(())
        }

        async fn channel_eof(
            &mut self,
            channel: ChannelId,
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            if self
                .evidence
                .exec_stdin_channels
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&channel)
            {
                session.extended_data(channel, 1, b"stdin-closed\n".as_slice())?;
                session.exit_status_request(channel, 0)?;
                session.eof(channel)?;
                session.close(channel)?;
            }
            Ok(())
        }

        async fn channel_close(
            &mut self,
            channel: ChannelId,
            _session: &mut Session,
        ) -> Result<(), Self::Error> {
            self.evidence
                .closed_channels
                .lock()
                .expect("closed channel evidence")
                .insert(channel);
            Ok(())
        }

        async fn data(
            &mut self,
            channel: ChannelId,
            data: &[u8],
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            if self
                .evidence
                .terminal_channels
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(&channel)
            {
                session.data(channel, data.to_vec())?;
            } else if self
                .evidence
                .sftp_channels
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(&channel)
            {
                let response: bytes::Bytes = SftpPacket::Version(SftpVersion::new())
                    .try_into()
                    .map_err(|_| russh::Error::Inconsistent)?;
                session.data(channel, response)?;
            }
            Ok(())
        }

        async fn window_change_request(
            &mut self,
            channel: ChannelId,
            columns: u32,
            rows: u32,
            _pixel_width: u32,
            _pixel_height: u32,
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            self.evidence
                .resizes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((columns, rows));
            session.channel_success(channel)?;
            Ok(())
        }
    }

    struct IsolatedServer {
        port: u16,
        host_key_blob: Vec<u8>,
        evidence: Arc<ServerEvidence>,
        accept_task: JoinHandle<()>,
    }

    impl IsolatedServer {
        async fn start(accepted_public_key: Option<Vec<u8>>, reject_pty: bool) -> Self {
            Self::start_with_direct_behavior(
                accepted_public_key,
                reject_pty,
                DirectTcpipBehavior::Forward,
            )
            .await
        }

        async fn start_with_direct_behavior(
            accepted_public_key: Option<Vec<u8>>,
            reject_pty: bool,
            direct_tcpip_behavior: DirectTcpipBehavior,
        ) -> Self {
            let host_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)
                .expect("generate host key");
            let host_key_blob = host_key.public_key().public_key_bytes();
            let config = Arc::new(server::Config {
                auth_rejection_time: Duration::ZERO,
                auth_rejection_time_initial: Some(Duration::ZERO),
                keys: vec![host_key],
                ..server::Config::default()
            });
            let evidence = Arc::new(ServerEvidence::default());
            let listener = TcpListener::bind(("127.0.0.1", 0))
                .await
                .expect("bind isolated server");
            let port = listener.local_addr().expect("local address").port();
            let server_evidence = Arc::clone(&evidence);
            let accept_task = tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        break;
                    };
                    server_evidence
                        .tcp_connections
                        .fetch_add(1, Ordering::SeqCst);
                    let handler = TestServerHandler {
                        evidence: Arc::clone(&server_evidence),
                        accepted_public_key: accepted_public_key.clone(),
                        reject_pty,
                        direct_tcpip_behavior,
                        keyboard_interactive_round: 0,
                    };
                    let config = Arc::clone(&config);
                    tokio::spawn(async move {
                        if let Ok(session) = server::run_stream(config, stream, handler).await {
                            let _ = session.await;
                        }
                    });
                }
            });
            Self {
                port,
                host_key_blob,
                evidence,
                accept_task,
            }
        }
    }

    impl Drop for IsolatedServer {
        fn drop(&mut self) {
            self.accept_task.abort();
        }
    }

    fn connect_request(server: &IsolatedServer) -> ConnectRequest {
        ConnectRequest::new("127.0.0.1", server.port).expect("valid connect request")
    }

    #[tokio::test]
    async fn verified_transport_uses_http_connect_ingress_for_russh_stream() {
        let server = IsolatedServer::start(None, false).await;
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind fake proxy");
        let proxy_port = listener.local_addr().expect("proxy address").port();
        let target_port = server.port;
        let proxy_task = tokio::spawn(async move {
            let (mut client, _) = listener.accept().await.expect("accept proxy client");
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(client.read_u8().await.expect("CONNECT request byte"));
            }
            assert!(request.starts_with(format!("CONNECT 127.0.0.1:{target_port} ").as_bytes()));
            let mut upstream = TcpStream::connect(("127.0.0.1", target_port))
                .await
                .expect("connect SSH server");
            client
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await
                .expect("CONNECT response");
            let _ = copy_bidirectional(&mut client, &mut upstream).await;
        });
        let verifier = Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted));
        let request = connect_request(&server).with_ingress(RouteIngress::HttpConnect {
            proxy: Endpoint::parse("127.0.0.1", proxy_port).expect("proxy endpoint"),
            credentials: None,
        });
        let transport = VerifiedTransport::connect(request, Arc::clone(&verifier))
            .await
            .expect("verified transport through CONNECT");
        assert_eq!(verifier.observations().len(), 1);
        drop(transport);
        proxy_task.abort();
    }

    async fn verified_transport(
        server: &IsolatedServer,
        verifier: Arc<RecordingVerifier>,
    ) -> VerifiedTransport<RecordingVerifier> {
        VerifiedTransport::connect(connect_request(server), verifier)
            .await
            .expect("verify host transport")
    }

    async fn password_transport(
        server: &IsolatedServer,
        verifier: Arc<RecordingVerifier>,
        password: &str,
    ) -> super::AuthenticatedTransport<RecordingVerifier> {
        verified_transport(server, verifier)
            .await
            .authenticate(
                TEST_USER,
                Authentication::password(password.as_bytes().to_vec()),
            )
            .await
            .expect("authenticate password")
    }

    async fn next_data<V: HostKeyVerifier>(shell: &mut RemoteShell<V>) -> bytes::Bytes {
        timeout(Duration::from_secs(5), async {
            loop {
                if let ShellEvent::Data(data) = shell
                    .next_event()
                    .await
                    .expect("remote shell remains connected")
                {
                    return data;
                }
            }
        })
        .await
        .expect("receive remote data")
    }

    #[test]
    fn authentication_debug_never_contains_authentication_material() {
        let authentication = Authentication::password(b"do-not-print".to_vec());
        let debug = format!("{authentication:?}");
        assert!(!debug.contains("do-not-print"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn proxy_credential_validation_maps_to_explicit_redacted_route_configuration_error() {
        let credential_error =
            ProxyCredentials::new("", b"do-not-print".to_vec()).expect_err("invalid username");
        let error = TransportError::from(credential_error);
        assert!(matches!(
            error,
            TransportError::RouteIngress(route_error)
                if route_error.stage == IngressStage::Configuration
                    && route_error.kind == IngressFailureKind::InvalidConfiguration
        ));
        assert!(!error.to_string().contains("do-not-print"));
    }

    #[tokio::test]
    async fn two_jump_chain_keeps_each_transport_and_verifies_each_host_independently() {
        let hop_one = IsolatedServer::start(None, false).await;
        let hop_two = IsolatedServer::start(None, false).await;
        let target = IsolatedServer::start(None, false).await;
        let hop_one_verifier = Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted));
        let hop_two_verifier = Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted));
        let target_verifier = Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted));

        let hop_one_transport =
            password_transport(&hop_one, Arc::clone(&hop_one_verifier), TEST_PASSWORD).await;
        let hop_two_stream = hop_one_transport
            .into_direct_tcpip_stream(
                Endpoint::parse("127.0.0.1", hop_two.port).expect("hop two endpoint"),
                Duration::from_secs(2),
            )
            .await
            .expect("open hop two channel");
        let hop_two_transport = VerifiedTransport::connect_stream(
            connect_request(&hop_two),
            hop_two_stream,
            Arc::clone(&hop_two_verifier),
        )
        .await
        .expect("verify hop two")
        .authenticate(
            TEST_USER,
            Authentication::password(TEST_PASSWORD.as_bytes().to_vec()),
        )
        .await
        .expect("authenticate hop two");
        let target_stream = hop_two_transport
            .into_direct_tcpip_stream(
                Endpoint::parse("127.0.0.1", target.port).expect("target endpoint"),
                Duration::from_secs(2),
            )
            .await
            .expect("open target channel");
        let target_transport = VerifiedTransport::connect_stream(
            connect_request(&target),
            target_stream,
            Arc::clone(&target_verifier),
        )
        .await
        .expect("verify target")
        .authenticate(
            TEST_USER,
            Authentication::password(TEST_PASSWORD.as_bytes().to_vec()),
        )
        .await
        .expect("authenticate target");
        let mut shell = target_transport
            .open_remote_shell(PtySize::new(100, 30, 0, 0).expect("PTY size"))
            .await
            .expect("open final shell");
        assert_eq!(
            next_data(&mut shell).await,
            bytes::Bytes::from_static(b"\x1b[31m\xe6\xac\xa2\xe8\xbf\x8e\x1b[0m\r\n")
        );
        shell.disconnect().await.expect("disconnect final shell");

        for (server, verifier) in [
            (&hop_one, &hop_one_verifier),
            (&hop_two, &hop_two_verifier),
            (&target, &target_verifier),
        ] {
            let observations = verifier.observations();
            assert_eq!(observations.len(), 1);
            assert_eq!(
                observations[0].0,
                Endpoint::parse("127.0.0.1", server.port).unwrap()
            );
            assert_eq!(observations[0].1.public_key_blob, server.host_key_blob);
            assert_eq!(server.evidence.auth_attempts.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn direct_tcpip_rejection_and_timeout_preserve_jump_stage() {
        for (behavior, expected_kind, open_timeout) in [
            (
                DirectTcpipBehavior::Reject,
                JumpFailureKind::Rejected,
                Duration::from_secs(2),
            ),
            (
                DirectTcpipBehavior::Stall,
                JumpFailureKind::Timeout,
                Duration::from_millis(50),
            ),
        ] {
            let hop = IsolatedServer::start_with_direct_behavior(None, false, behavior).await;
            let verifier = Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted));
            let transport = password_transport(&hop, verifier, TEST_PASSWORD).await;
            let error = transport
                .into_direct_tcpip_stream(
                    Endpoint::parse("127.0.0.1", 9).expect("discard endpoint"),
                    open_timeout,
                )
                .await
                .expect_err("jump channel must fail");
            assert!(matches!(
                error,
                TransportError::Jump(jump_error)
                    if jump_error.stage == JumpStage::DirectTcpipOpen
                        && jump_error.kind == expected_kind
            ));
        }
    }

    #[tokio::test]
    async fn dropping_parent_owned_direct_stream_closes_forwarded_connection() {
        let target_listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind target");
        let target_port = target_listener.local_addr().expect("target address").port();
        let (closed_sender, closed_receiver) = oneshot::channel();
        let target_task = tokio::spawn(async move {
            let (mut connection, _) = target_listener.accept().await.expect("accept target");
            let mut byte = [0_u8; 1];
            connection
                .read_exact(&mut byte)
                .await
                .expect("forwarded byte");
            assert_eq!(byte, [b'x']);
            let result = connection.read(&mut byte).await.expect("forwarded close");
            assert_eq!(result, 0);
            let _ = closed_sender.send(());
        });
        let hop = IsolatedServer::start(None, false).await;
        let verifier = Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted));
        let transport = password_transport(&hop, verifier, TEST_PASSWORD).await;
        let mut stream = transport
            .into_direct_tcpip_stream(
                Endpoint::parse("127.0.0.1", target_port).expect("target endpoint"),
                Duration::from_secs(2),
            )
            .await
            .expect("open forwarded stream");
        stream.write_all(b"x").await.expect("write forwarded byte");
        drop(stream);
        timeout(Duration::from_secs(2), closed_receiver)
            .await
            .expect("forwarded close timeout")
            .expect("close signal");
        target_task.await.expect("target task");
    }

    #[test]
    fn rejects_empty_usernames_and_zero_sized_remote_ptys() {
        assert!(matches!(
            PtySize::new(0, 24, 0, 0),
            Err(TransportError::InvalidPtySize)
        ));
        assert!(matches!(
            super::validate_username(""),
            Err(TransportError::InvalidUsername)
        ));
    }

    #[tokio::test]
    async fn disconnect_deadline_bounds_a_stalled_initial_close_step() {
        let result = super::complete_disconnect_within(
            Duration::from_millis(20),
            pending::<super::Result<()>>(),
        )
        .await;
        assert!(matches!(result, Err(TransportError::DisconnectTimeout)));
    }

    #[test]
    fn default_disconnect_deadline_is_one_second() {
        assert_eq!(
            super::ConnectionTimeouts::default().disconnect,
            Duration::from_secs(1)
        );
    }

    #[tokio::test]
    async fn phases_verify_then_authenticate_then_open_remote_shell() {
        let server = IsolatedServer::start(None, false).await;
        let verifier = Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted));
        let verified = verified_transport(&server, Arc::clone(&verifier)).await;
        assert_eq!(server.evidence.auth_attempts.load(Ordering::SeqCst), 0);
        assert!(
            server
                .evidence
                .pty_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
        let authenticated = verified
            .authenticate(
                TEST_USER,
                Authentication::password(TEST_PASSWORD.as_bytes().to_vec()),
            )
            .await
            .expect("authenticate only after host verification");
        assert_eq!(server.evidence.auth_attempts.load(Ordering::SeqCst), 1);
        assert!(
            server
                .evidence
                .pty_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
        let mut shell = authenticated
            .open_remote_shell(PtySize::new(80, 24, 0, 0).expect("valid size"))
            .await
            .expect("open remote shell only after authentication");

        assert_eq!(
            next_data(&mut shell).await.as_ref(),
            b"\x1b[31m\xe6\xac\xa2\xe8\xbf\x8e\x1b[0m\r\n"
        );
        shell
            .send_input(bytes::Bytes::from_static(b"echo \xff\r"))
            .await
            .expect("send raw input");
        assert_eq!(next_data(&mut shell).await.as_ref(), b"echo \xff\r");
        shell
            .resize(PtySize::new(132, 43, 1000, 700).expect("valid resize"))
            .await
            .expect("resize remote PTY");

        timeout(Duration::from_secs(5), async {
            loop {
                if server
                    .evidence
                    .resizes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .contains(&(132, 43))
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("server observes resize");
        let pty_requests = server
            .evidence
            .pty_requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(pty_requests, vec![("xterm-256color".to_owned(), 80, 24)]);
        let observations = verifier.observations();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].0.normalized_address(), "127.0.0.1");
        assert_eq!(observations[0].0.port(), server.port);
        assert_eq!(observations[0].1.public_key_blob, server.host_key_blob);
        shell.disconnect().await.expect("disconnect cleanly");
    }

    #[tokio::test]
    async fn sftp_transport_opens_a_real_subsystem_without_a_pty_or_shell() {
        let server = IsolatedServer::start(None, false).await;
        let verifier = Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted));
        let transport = password_transport(&server, verifier, TEST_PASSWORD).await;
        let sftp = transport.open_sftp().await.expect("open SFTP subsystem");
        assert!(
            server
                .evidence
                .pty_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
        sftp.disconnect().await.expect("disconnect SFTP transport");
    }

    #[tokio::test]
    async fn passive_close_handle_observes_sftp_transport_shutdown_without_a_ping() {
        let server = IsolatedServer::start(None, false).await;
        let verifier = Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted));
        let authenticated = password_transport(&server, verifier, TEST_PASSWORD).await;
        let mut closed = authenticated.close_handle();
        assert!(!closed.is_closed());
        let sftp = authenticated
            .open_sftp()
            .await
            .expect("open SFTP subsystem");
        sftp.disconnect().await.expect("disconnect SFTP transport");
        timeout(Duration::from_secs(2), closed.wait_closed())
            .await
            .expect("passive close observation timeout");
        assert!(closed.is_closed());
    }

    #[tokio::test]
    async fn heartbeat_handle_waits_for_reply_and_rejects_a_closed_transport() {
        let server = IsolatedServer::start(None, false).await;
        let verifier = Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted));
        let authenticated = password_transport(&server, verifier, TEST_PASSWORD).await;
        let heartbeat = authenticated.heartbeat_handle();
        timeout(Duration::from_secs(2), heartbeat.request_reply())
            .await
            .expect("keepalive reply timeout")
            .expect("REQUEST_FAILURE is still a liveness reply");

        let shell = authenticated
            .open_remote_shell(PtySize::new(80, 24, 0, 0).expect("valid size"))
            .await
            .expect("open remote shell");
        shell.disconnect().await.expect("disconnect remote shell");
        assert!(
            timeout(Duration::from_secs(2), heartbeat.request_reply())
                .await
                .expect("closed transport result timeout")
                .is_err()
        );
    }

    #[tokio::test]
    async fn shared_channels_keep_parent_pty_interactive_with_one_tcp_and_authentication() {
        let server = IsolatedServer::start(None, false).await;
        let authenticated = password_transport(
            &server,
            Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted)),
            TEST_PASSWORD,
        )
        .await;
        let channels = authenticated.shared_channels();
        let mut parent = authenticated
            .open_remote_shell(PtySize::new(80, 24, 0, 0).unwrap())
            .await
            .unwrap();
        let _ = next_data(&mut parent).await;
        let (_cancel, cancelled) = watch::channel(false);
        let input = vec![b'x'; 96 * 1024];
        let output = channels
            .execute_capture_request(
                b"stdin-echo",
                Some(&input),
                Duration::from_secs(5),
                128 * 1024,
                || true,
                cancelled,
            )
            .await
            .unwrap();
        assert_eq!(output.stdout, input);
        assert_eq!(output.exit_status, Some(0));
        let mut child = channels
            .open_terminal_command(PtySize::new(100, 30, 0, 0).unwrap(), "stall-exec", || true)
            .await
            .unwrap();
        child
            .send_input(Bytes::from_static(b"child-input\r"))
            .await
            .unwrap();
        let data = timeout(Duration::from_secs(5), async {
            loop {
                if let ShellEvent::Data(data) = child.next_event().await.unwrap() {
                    break data;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(data.as_ref(), b"child-input\r");
        parent
            .send_input(Bytes::from_static(b"parent-concurrent\r"))
            .await
            .unwrap();
        assert_eq!(
            next_data(&mut parent).await.as_ref(),
            b"parent-concurrent\r"
        );
        child
            .resize(PtySize::new(120, 40, 0, 0).unwrap())
            .await
            .unwrap();
        child.disconnect().await.unwrap();
        parent
            .send_input(Bytes::from_static(b"parent-after-child-close\r"))
            .await
            .unwrap();
        assert_eq!(
            next_data(&mut parent).await.as_ref(),
            b"parent-after-child-close\r"
        );
        let sftp = channels
            .open_sftp_channel()
            .await
            .expect("SFTP on the same authenticated transport");
        sftp.close().await.unwrap();
        parent
            .send_input(Bytes::from_static(b"parent-after-sftp-close\r"))
            .await
            .unwrap();
        assert_eq!(
            next_data(&mut parent).await.as_ref(),
            b"parent-after-sftp-close\r"
        );
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let echo = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (mut read, mut write) = socket.split();
            tokio::io::copy(&mut read, &mut write).await.unwrap();
        });
        let mut forward = channels
            .open_direct_tcpip("127.0.0.1", port, "127.0.0.1", 1025)
            .await
            .unwrap();
        forward.write_all(b"forward-echo").await.unwrap();
        let mut forwarded = [0; 12];
        timeout(Duration::from_secs(5), forward.read_exact(&mut forwarded))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&forwarded, b"forward-echo");
        forward.shutdown().await.unwrap();
        drop(forward);
        timeout(Duration::from_secs(5), echo)
            .await
            .unwrap()
            .unwrap();
        parent
            .send_input(Bytes::from_static(b"parent-after-forward-close\r"))
            .await
            .unwrap();
        assert_eq!(
            next_data(&mut parent).await.as_ref(),
            b"parent-after-forward-close\r"
        );
        assert_eq!(server.evidence.tcp_connections.load(Ordering::SeqCst), 1);
        assert_eq!(server.evidence.auth_attempts.load(Ordering::SeqCst), 1);
        assert_eq!(server.evidence.shell_requests.load(Ordering::SeqCst), 1);
        assert_eq!(server.evidence.pty_requests.lock().unwrap().len(), 2);
        assert!(!channels.is_closed());
        parent.disconnect().await.unwrap();
        timeout(Duration::from_secs(5), channels.wait_closed())
            .await
            .unwrap();
        assert!(channels.is_closed());
    }

    #[tokio::test]
    async fn shared_exec_timeout_cancel_and_output_limit_close_only_the_child() {
        let server = IsolatedServer::start(None, false).await;
        let authenticated = password_transport(
            &server,
            Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted)),
            TEST_PASSWORD,
        )
        .await;
        let channels = authenticated.shared_channels();
        let mut parent = authenticated
            .open_remote_shell(PtySize::new(80, 24, 0, 0).unwrap())
            .await
            .unwrap();
        let _ = next_data(&mut parent).await;
        let (_cancel, cancelled) = watch::channel(false);
        assert!(matches!(
            channels
                .execute_capture_request(
                    b"stall-exec",
                    None,
                    Duration::from_millis(40),
                    128,
                    || true,
                    cancelled.clone()
                )
                .await,
            Err(TransportError::RemoteExecTimeout)
        ));
        assert!(matches!(
            channels
                .execute_capture_request(
                    b"too-much-output",
                    None,
                    Duration::from_secs(2),
                    4,
                    || true,
                    cancelled
                )
                .await,
            Err(TransportError::RemoteExecOutputTooLarge)
        ));
        let (cancel, cancelled) = watch::channel(false);
        let task_channels = channels.clone();
        let task = tokio::spawn(async move {
            task_channels
                .execute_capture_request(
                    b"stall-exec",
                    None,
                    Duration::from_secs(5),
                    128,
                    || true,
                    cancelled,
                )
                .await
        });
        timeout(Duration::from_secs(5), async {
            while server.evidence.exec_requests.lock().unwrap().len() < 3 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        cancel.send_replace(true);
        assert!(matches!(
            task.await.unwrap(),
            Err(TransportError::RemoteExecRejected)
        ));
        timeout(Duration::from_secs(5), async {
            while server.evidence.closed_channels.lock().unwrap().len() < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("timeout and cancelled stalled children closed on the server");
        parent
            .send_input(Bytes::from_static(b"still-interactive\r"))
            .await
            .unwrap();
        assert_eq!(
            next_data(&mut parent).await.as_ref(),
            b"still-interactive\r"
        );
        assert_eq!(server.evidence.tcp_connections.load(Ordering::SeqCst), 1);
        assert_eq!(server.evidence.auth_attempts.load(Ordering::SeqCst), 1);
        assert!(!channels.is_closed());
        parent.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn shared_exec_and_child_pty_recheck_admission_before_dispatch() {
        let server = IsolatedServer::start(None, false).await;
        let authenticated = password_transport(
            &server,
            Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted)),
            TEST_PASSWORD,
        )
        .await;
        let channels = authenticated.shared_channels();
        let mut parent = authenticated
            .open_remote_shell(PtySize::new(80, 24, 0, 0).unwrap())
            .await
            .unwrap();
        let _ = next_data(&mut parent).await;
        let (_cancel, cancelled) = watch::channel(false);
        assert!(matches!(
            channels
                .execute_capture_request(
                    b"must-not-run",
                    None,
                    Duration::from_secs(2),
                    128,
                    || false,
                    cancelled
                )
                .await,
            Err(TransportError::RemoteExecRejected)
        ));
        assert!(matches!(
            channels
                .open_terminal_command(PtySize::new(80, 24, 0, 0).unwrap(), "must-not-run", || {
                    false
                })
                .await,
            Err(TransportError::RemoteExecRejected)
        ));
        assert!(server.evidence.exec_requests.lock().unwrap().is_empty());
        parent
            .send_input(Bytes::from_static(b"admission-denied-parent-ok\r"))
            .await
            .unwrap();
        assert_eq!(
            next_data(&mut parent).await.as_ref(),
            b"admission-denied-parent-ok\r"
        );
        parent.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn exec_transport_uses_fresh_non_pty_channels_and_bounds_output() {
        let server = IsolatedServer::start(None, false).await;
        let verifier = Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted));
        let authenticated = password_transport(&server, verifier, TEST_PASSWORD).await;
        let mut transport = authenticated.into_exec_transport();

        let first = transport
            .execute_capture(b"metrics-provider-v1", Duration::from_secs(5), 128)
            .await
            .expect("capture fixed metrics output");
        assert_eq!(first.stdout, b"metric-out\n");
        assert_eq!(first.stderr, b"metric-warning\n");
        assert_eq!(first.exit_status, Some(0));
        assert!(
            server
                .evidence
                .pty_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );

        let second = transport
            .execute_capture(b"metrics-provider-v1", Duration::from_secs(5), 4)
            .await
            .expect_err("oversized output poisons the metrics transport");
        assert!(matches!(second, TransportError::RemoteExecOutputTooLarge));
        let poisoned = transport
            .execute_capture(b"metrics-provider-v1", Duration::from_secs(5), 128)
            .await
            .expect_err("poisoned transport cannot be reused");
        assert!(matches!(poisoned, TransportError::ConnectionLost));
        assert_eq!(
            server
                .evidence
                .exec_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            [
                b"metrics-provider-v1".to_vec(),
                b"metrics-provider-v1".to_vec()
            ]
        );
        transport
            .disconnect()
            .await
            .expect("poisoned disconnect is idempotent");
    }

    #[tokio::test]
    async fn dynamic_exec_requires_final_admission_before_sending_any_command() {
        let server = IsolatedServer::start(None, false).await;
        let transport = password_transport(
            &server,
            Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted)),
            TEST_PASSWORD,
        )
        .await;
        let mut transport = transport.into_exec_transport();
        let command = String::from("dynamic-operation");
        assert!(matches!(
            transport
                .execute_capture_request(
                    command.as_bytes(),
                    None,
                    Duration::from_secs(2),
                    128,
                    || false
                )
                .await,
            Err(TransportError::RemoteExecRejected)
        ));
        assert!(
            server
                .evidence
                .exec_requests
                .lock()
                .expect("exec evidence")
                .is_empty()
        );
        let output = transport
            .execute_capture_request(
                command.as_bytes(),
                None,
                Duration::from_secs(2),
                128,
                || true,
            )
            .await
            .expect("approved dynamic exec");
        assert_eq!(output.exit_status, Some(0));
        assert_eq!(
            *server.evidence.exec_requests.lock().expect("exec evidence"),
            vec![command.into_bytes()]
        );
        assert!(
            server
                .evidence
                .pty_requests
                .lock()
                .expect("PTY evidence")
                .is_empty()
        );
        transport.disconnect().await.expect("disconnect");
    }

    #[tokio::test]
    async fn dynamic_exec_sends_bounded_stdin_and_drains_output_without_a_pty() {
        let server = IsolatedServer::start(None, false).await;
        let transport = password_transport(
            &server,
            Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted)),
            TEST_PASSWORD,
        )
        .await;
        let mut transport = transport.into_exec_transport();
        let input = vec![b'x'; 96 * 1024];
        let output = transport
            .execute_capture_request(
                b"stdin-echo",
                Some(&input),
                Duration::from_secs(5),
                128 * 1024,
                || true,
            )
            .await
            .expect("stdin and output");
        assert_eq!(output.stdout, input);
        assert_eq!(output.exit_status, Some(0));
        assert!(
            server
                .evidence
                .pty_requests
                .lock()
                .expect("PTY evidence")
                .is_empty()
        );
        transport.disconnect().await.expect("disconnect");
    }

    #[tokio::test]
    async fn isolated_exec_stream_uses_dedicated_transport_and_reports_eof_then_exit() {
        let server = IsolatedServer::start(None, false).await;
        let transport = password_transport(
            &server,
            Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted)),
            TEST_PASSWORD,
        )
        .await;
        let mut stream = transport
            .into_exec_transport()
            .open_stream(
                b"stdin-echo",
                RemoteExecStreamLimits {
                    max_stdin_bytes: 1024,
                    max_stdout_bytes: 1024,
                    max_stderr_bytes: 1024,
                },
                || true,
            )
            .await
            .expect("open a non-PTY exec channel");
        stream.send_stdin(b"stream-input").await.expect("stdin ACK");
        stream.close_stdin().await.expect("EOF ACK");

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit = None;
        while let Some(event) = stream.next_event().await.expect("stream event") {
            match event {
                RemoteExecStreamEvent::Stdout(bytes) => stdout.extend_from_slice(&bytes),
                RemoteExecStreamEvent::Stderr(bytes) => stderr.extend_from_slice(&bytes),
                RemoteExecStreamEvent::ExitStatus(code) => exit = code,
                RemoteExecStreamEvent::Eof => break,
            }
        }
        assert_eq!(stdout, b"stream-input");
        assert_eq!(stderr, b"stdin-closed\n");
        assert_eq!(exit, Some(0));
        assert!(
            server
                .evidence
                .pty_requests
                .lock()
                .expect("PTY evidence")
                .is_empty()
        );
        stream
            .disconnect()
            .await
            .expect("independent transport cleanup");

        let transport = password_transport(
            &server,
            Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted)),
            TEST_PASSWORD,
        )
        .await;
        let stream = transport
            .into_exec_transport()
            .open_stream(
                b"stall-exec",
                RemoteExecStreamLimits {
                    max_stdin_bytes: 1024,
                    max_stdout_bytes: 1024,
                    max_stderr_bytes: 1024,
                },
                || true,
            )
            .await
            .expect("open cancellable independent exec");
        // Cancellation is a transport disconnect, not a channel reuse or a PTY fallback. The
        // fixture's `disconnect` ACK proves the dedicated client transport completed teardown.
        stream
            .disconnect()
            .await
            .expect("cancel cleanup disconnect");
        assert!(
            server
                .evidence
                .pty_requests
                .lock()
                .expect("PTY evidence")
                .is_empty()
        );
        assert_eq!(server.evidence.tcp_connections.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn dynamic_exec_timeout_poisons_its_dedicated_transport() {
        let server = IsolatedServer::start(None, false).await;
        let transport = password_transport(
            &server,
            Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted)),
            TEST_PASSWORD,
        )
        .await;
        let mut transport = transport.into_exec_transport();
        assert!(matches!(
            transport
                .execute_capture_request(
                    b"stall-exec",
                    None,
                    Duration::from_millis(30),
                    128,
                    || true
                )
                .await,
            Err(TransportError::RemoteExecTimeout)
        ));
        assert!(matches!(
            transport
                .execute_capture(b"metrics-provider-v1", Duration::from_secs(1), 128)
                .await,
            Err(TransportError::ConnectionLost)
        ));
        transport.disconnect().await.expect("poisoned cleanup");
    }

    #[tokio::test]
    async fn host_key_rejection_and_mismatch_happen_before_authentication() {
        for decision in [
            HostKeyDecision::Rejected,
            HostKeyDecision::Mismatch {
                trusted_fingerprint: "SHA256:trusted".to_owned(),
            },
        ] {
            let server = IsolatedServer::start(None, false).await;
            let verifier = Arc::new(RecordingVerifier::new(decision.clone()));
            let error = VerifiedTransport::connect(connect_request(&server), Arc::clone(&verifier))
                .await
                .expect_err("host key must block connection");
            match decision {
                HostKeyDecision::Rejected => {
                    assert!(matches!(error, TransportError::HostKeyRejected));
                }
                HostKeyDecision::Mismatch { .. } => {
                    assert!(matches!(error, TransportError::HostKeyMismatch { .. }));
                }
                HostKeyDecision::Trusted => unreachable!(),
            }
            assert_eq!(server.evidence.auth_attempts.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn authentication_rejection_does_not_open_a_remote_pty() {
        let server = IsolatedServer::start(None, false).await;
        let verifier = Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted));
        let error = verified_transport(&server, verifier)
            .await
            .authenticate(TEST_USER, Authentication::password(b"wrong".to_vec()))
            .await
            .expect_err("wrong password must fail");
        assert!(matches!(error, TransportError::AuthenticationRejected));
        assert!(
            server
                .evidence
                .pty_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "requires an isolated NORISHELL_M4_AGENT_QA ssh-agent and sshd fixture"]
    async fn configured_macos_agent_certificate_authenticates_and_opens_a_real_remote_pty() {
        assert!(std::env::var_os("NORISHELL_M4_AGENT_QA").is_some());
        let socket = std::env::var_os("SSH_AUTH_SOCK").expect("isolated QA Agent socket");
        let stream = tokio::net::UnixStream::connect(socket)
            .await
            .expect("connect isolated QA Agent");
        let mut client = AgentClient::connect(stream);
        let certificate = client
            .request_identities()
            .await
            .expect("list isolated QA Agent")
            .into_iter()
            .find_map(|identity| match identity {
                AgentIdentity::Certificate { certificate, .. } => Some(certificate),
                AgentIdentity::PublicKey { .. } => None,
            })
            .expect("isolated QA Agent certificate");
        let port = std::env::var("NORISHELL_M4_SSH_PORT")
            .expect("isolated QA sshd port")
            .parse::<u16>()
            .expect("numeric QA port");
        let username = std::env::var("NORISHELL_M4_SSH_USER").expect("isolated QA sshd user");
        let request = ConnectRequest::new("127.0.0.1", port).expect("QA endpoint");
        let verifier = Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted));
        let authenticated = VerifiedTransport::connect(request, verifier)
            .await
            .expect("connect and verify isolated sshd")
            .authenticate(
                username,
                Authentication::ssh_agent_certificate(certificate, client),
            )
            .await
            .expect("authenticate with exact Agent certificate");
        let mut shell = authenticated
            .open_remote_shell(PtySize::new(80, 24, 0, 0).expect("valid PTY"))
            .await
            .expect("open real remote PTY");
        shell
            .send_input("printf NORISHELL_CERT_PTY_OK\\n; exit\n")
            .await
            .expect("send bounded QA command");
        let output = timeout(Duration::from_secs(5), async {
            let mut bytes = Vec::new();
            loop {
                match shell.next_event().await.expect("read QA shell event") {
                    ShellEvent::Data(data) | ShellEvent::ExtendedData { data, .. } => {
                        bytes.extend_from_slice(&data);
                    }
                    ShellEvent::Closed | ShellEvent::Eof => break bytes,
                    _ => {}
                }
            }
        })
        .await
        .expect("QA shell closes within deadline");
        assert!(
            String::from_utf8_lossy(&output).contains("NORISHELL_CERT_PTY_OK"),
            "remote PTY did not return the certificate-authenticated marker"
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "requires an isolated NORISHELL_M4_AGENT_QA ssh-agent and sshd fixture"]
    async fn configured_macos_agent_certificate_with_wrong_principal_is_rejected_by_sshd() {
        assert!(std::env::var_os("NORISHELL_M4_AGENT_QA").is_some());
        let socket = std::env::var_os("SSH_AUTH_SOCK").expect("isolated QA Agent socket");
        let stream = tokio::net::UnixStream::connect(socket)
            .await
            .expect("connect isolated QA Agent");
        let mut client = AgentClient::connect(stream);
        let certificate = client
            .request_identities()
            .await
            .expect("list isolated QA Agent")
            .into_iter()
            .find_map(|identity| match identity {
                AgentIdentity::Certificate { certificate, .. }
                    if certificate.key_id() == "wrong-principal" =>
                {
                    Some(certificate)
                }
                _ => None,
            })
            .expect("wrong-principal QA certificate");
        let port = std::env::var("NORISHELL_M4_SSH_PORT")
            .expect("isolated QA sshd port")
            .parse::<u16>()
            .expect("numeric QA port");
        let username = std::env::var("NORISHELL_M4_SSH_USER").expect("isolated QA sshd user");
        let request = ConnectRequest::new("127.0.0.1", port).expect("QA endpoint");
        let verifier = Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted));
        let error = VerifiedTransport::connect(request, verifier)
            .await
            .expect("connect and verify isolated sshd")
            .authenticate(
                username,
                Authentication::ssh_agent_certificate(certificate, client),
            )
            .await
            .expect_err("sshd must reject a certificate without the requested principal");
        assert!(matches!(error, TransportError::AuthenticationRejected));
    }

    #[test]
    fn noninteractive_partial_success_is_preserved_as_a_typed_terminal_error() {
        let error = super::authentication_failure(true)
            .expect_err("partial success must stop the ordered fallback chain");
        assert!(
            matches!(error, TransportError::AuthenticationIncomplete),
            "unexpected authentication result: {error:?}"
        );
    }

    #[tokio::test]
    async fn keyboard_interactive_preserves_two_rounds_and_authenticates_only_after_final_answer() {
        let server = IsolatedServer::start(None, false).await;
        let verifier = Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted));
        let mut transport = verified_transport(&server, verifier).await;
        let first = transport
            .keyboard_interactive_start(TEST_USER)
            .await
            .expect("start keyboard-interactive");
        let super::KeyboardInteractiveOutcome::Challenge(first) = first else {
            panic!("expected first keyboard-interactive round");
        };
        assert_eq!(first.prompts.len(), 2);
        assert!(first.prompts[0].echo);
        assert!(!first.prompts[1].echo);
        let second = transport
            .keyboard_interactive_respond(vec![
                zeroize::Zeroizing::new(TEST_USER.to_owned()),
                zeroize::Zeroizing::new("111111".to_owned()),
            ])
            .await
            .expect("answer first round");
        let super::KeyboardInteractiveOutcome::Challenge(second) = second else {
            panic!("expected second keyboard-interactive round");
        };
        assert_eq!(second.prompts.len(), 1);
        let final_outcome = transport
            .keyboard_interactive_respond(vec![zeroize::Zeroizing::new("yes".to_owned())])
            .await
            .expect("answer final round");
        assert!(matches!(
            final_outcome,
            super::KeyboardInteractiveOutcome::Success
        ));
        transport
            .into_authenticated()
            .expect("final round marks the transport authenticated");
        assert_eq!(server.evidence.auth_attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn copied_private_keys_work_with_and_without_passphrases() {
        for passphrase in [None, Some("vault-protected-passphrase")] {
            let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)
                .expect("generate client key");
            let public_key_blob = key.public_key().public_key_bytes();
            let encoded = match passphrase {
                Some(passphrase) => key
                    .encrypt(&mut rand::rng(), passphrase.as_bytes())
                    .expect("encrypt private key")
                    .to_openssh(LineEnding::LF)
                    .expect("encode encrypted private key"),
                None => key.to_openssh(LineEnding::LF).expect("encode private key"),
            };
            let metadata = inspect_private_key(encoded.as_bytes(), passphrase.map(str::as_bytes))
                .expect("inspect copied private key");
            assert_eq!(metadata.algorithm, "ssh-ed25519");
            assert_eq!(
                metadata.fingerprint_sha256,
                ssh_sha256_fingerprint(&public_key_blob)
            );
            let server = IsolatedServer::start(Some(public_key_blob), false).await;
            if passphrase.is_some() {
                let verifier = Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted));
                let error = verified_transport(&server, verifier)
                    .await
                    .authenticate(
                        TEST_USER,
                        Authentication::private_key(
                            encoded.as_bytes().to_vec(),
                            Some(b"wrong-passphrase".to_vec()),
                        ),
                    )
                    .await
                    .expect_err("wrong private-key passphrase must fail closed");
                assert!(matches!(error, TransportError::InvalidPrivateKey));
                assert_eq!(server.evidence.auth_attempts.load(Ordering::SeqCst), 0);
            }
            let verifier = Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted));
            let authenticated = verified_transport(&server, verifier)
                .await
                .authenticate(
                    TEST_USER,
                    Authentication::private_key(
                        encoded.as_bytes().to_vec(),
                        passphrase.map(|value| value.as_bytes().to_vec()),
                    ),
                )
                .await
                .expect("authenticate copied private key");
            let mut shell = authenticated
                .open_remote_shell(PtySize::new(90, 30, 0, 0).expect("valid size"))
                .await
                .expect("open remote shell");
            assert!(!next_data(&mut shell).await.is_empty());
            shell.disconnect().await.expect("disconnect cleanly");
        }
    }

    #[tokio::test]
    async fn rejected_pty_request_fails_without_opening_a_remote_shell() {
        let server = IsolatedServer::start(None, true).await;
        let verifier = Arc::new(RecordingVerifier::new(HostKeyDecision::Trusted));
        let authenticated = password_transport(&server, verifier, TEST_PASSWORD).await;
        let error = authenticated
            .open_remote_shell(PtySize::new(80, 24, 0, 0).expect("valid size"))
            .await
            .expect_err("rejected PTY request must fail closed");
        assert!(matches!(error, TransportError::PtyRejected));
        assert_eq!(
            server
                .evidence
                .pty_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            &[("xterm-256color".to_owned(), 80, 24)]
        );
    }
}
