//! Bounded Telnet protocol handling and a direct-TCP terminal transport.
//!
//! This crate intentionally has no Host, Known Hosts, Vault, credential,
//! RoutePlan, login-automation, heartbeat, persistence, or Tauri dependency.
//! A caller must provide a fresh cleartext-risk acknowledgement for every
//! connection attempt.

mod protocol;
mod transport;

pub use protocol::{
    DecodeResult, NegotiatedOption, ProtocolError, TelnetCodec, encode_naws, encode_user_input,
};
pub use transport::{
    DEFAULT_TELNET_PORT, TelnetConnectConfig, TelnetConnection, TelnetEvent, TelnetRuntimeError,
};
