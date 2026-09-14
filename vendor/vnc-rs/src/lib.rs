// Modified for NoriShell; see vendor/README.md at the repository root for upstream provenance.
//! Hardened RFB client transport and codecs vendored for NoriShell.
//!
//! This fork exposes only the bounded, cancellation-aware `HardenedVncClient`
//! used by NoriShell's desktop-protocol adapter. It supports RFB 3.3, 3.7,
//! and 3.8 with explicit None or VNC authentication, validates untrusted
//! lengths before allocation, and does not surface peer-provided diagnostic
//! strings. The historical upstream connector remains source-only for code
//! provenance and is intentionally not compiled or exported.

pub mod client;
mod codec;
pub mod config;
pub mod error;
pub mod event;

pub use client::{HardenedVncClient, HardenedVncOptions};
pub use config::*;
pub use error::*;
pub use event::*;

/// The active hardened client limits dimensions before any decoder allocation.
pub const MAX_DIMENSION: u16 = 8_192;
pub const MAX_PIXELS: usize = 16_777_216;
pub const MAX_FRAME_BYTES: usize = MAX_PIXELS * 4;
pub const MAX_WIRE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_TEXT_BYTES: usize = 65_536;
