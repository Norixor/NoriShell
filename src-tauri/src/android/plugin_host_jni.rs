//! Plugin Host entry point for the isolated `PluginHostService` process.
//!
//! The isolated process never runs the app `.setup` (no Tauri, no Core
//! services), so this is exported directly by the app crate instead of being a
//! runtime-registered bridge callback. It only runs the versioned frame loop
//! from `plugin_host_entry` over the socket the app process handed over.

use std::{
    ffi::c_void,
    os::{
        fd::{FromRawFd, OwnedFd},
        unix::net::UnixStream,
    },
};

use tauri_plugin_norishell_android as bridge;

use crate::plugin_host_process::{android_host::HostSocket, set_limit};

const EXIT_OK: i32 = 0;
/// Frame loop ended on a protocol or channel error (same code as desktop).
const EXIT_PROTOCOL: i32 = 65;
/// The fd was not a usable socket (EX_USAGE).
const EXIT_INVALID_CHANNEL: i32 = 64;
/// Panic inside the loop (EX_SOFTWARE).
const EXIT_PANIC: i32 = 70;
/// Resource limits could not be applied (EX_OSERR).
const EXIT_LIMITS: i32 = 71;

/// `NoriShellNative.nativeRunPluginHost(fd: Int): Int`
///
/// Takes ownership of `fd` (Kotlin detached it from its ParcelFileDescriptor)
/// and blocks until the parent sends Shutdown, closes the channel, or breaks
/// the protocol. The Kotlin service kills its own process when this returns.
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_norishell_android_bridge_NoriShellNative_nativeRunPluginHost(
    _env: *mut c_void,
    _class: *mut c_void,
    fd: i32,
) -> i32 {
    std::panic::catch_unwind(|| run(fd)).unwrap_or(EXIT_PANIC)
}

fn run(fd: i32) -> i32 {
    if fd < 0 || !is_socket(fd) {
        return EXIT_INVALID_CHANNEL;
    }
    // SAFETY: Kotlin transferred sole ownership of this open descriptor.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let stream = UnixStream::from(owned);
    // Defense-in-depth quotas that are safe inside ART. RLIMIT_NOFILE is not
    // lowered: Binder and ART need descriptors beyond a desktop-sized budget.
    if set_limit(libc::RLIMIT_CORE, 0).is_err() || set_limit(libc::RLIMIT_FSIZE, 0).is_err() {
        return EXIT_LIMITS;
    }
    let mut writer = HostSocket::new(stream);
    let Ok(mut reader) = writer.try_clone() else {
        return EXIT_INVALID_CHANNEL;
    };
    match crate::plugin_host_entry::run_with_streams(&mut reader, &mut writer) {
        Ok(()) => EXIT_OK,
        Err(_) => {
            bridge::log::info("plugin host loop ended on a channel or protocol error");
            EXIT_PROTOCOL
        }
    }
}

fn is_socket(fd: i32) -> bool {
    // SAFETY: `fstat` only writes into the provided zeroed struct.
    let mut status: libc::stat = unsafe { std::mem::zeroed() };
    unsafe {
        libc::fstat(fd, &raw mut status) == 0 && (status.st_mode & libc::S_IFMT) == libc::S_IFSOCK
    }
}
