//! NoriShell Android platform bridge.
//!
//! Rust -> Kotlin calls go through [`run_command`], which forwards to the
//! `NoriShellPlugin` Kotlin class registered by [`init`]. Kotlin -> Rust calls
//! arrive through the `NoriShellNative` JNI symbols exported here and are
//! dispatched to callbacks the application registers at setup (see
//! [`registry`]). The crate never depends on application types; every payload
//! is JSON text or raw bytes.
//!
//! The whole API only exists on Android. Desktop builds compile an empty crate
//! so `cargo clippy --workspace` keeps working.

#[cfg(target_os = "android")]
mod device_key;
#[cfg(target_os = "android")]
mod foreground;
#[cfg(target_os = "android")]
mod handle;
#[cfg(target_os = "android")]
mod jni_exports;
#[cfg(target_os = "android")]
pub mod log;
#[cfg(target_os = "android")]
mod notifications;
#[cfg(target_os = "android")]
pub mod registry;

#[cfg(target_os = "android")]
pub use device_key::{DeviceKeyError, device_key_delete, device_key_unwrap, device_key_wrap};
#[cfg(target_os = "android")]
pub use foreground::{
    ForegroundCounts, foreground_stop, foreground_update, request_notification_permission,
};
#[cfg(target_os = "android")]
pub use handle::{BridgeError, init, is_ready, run_command, verifier_initialized};
#[cfg(target_os = "android")]
pub use notifications::activity_resumed;
