//! Android-only application glue for the `tauri-plugin-norishell-android`
//! bridge: plugin registration, Kotlin -> Rust callbacks owned by the app,
//! and the session foreground-service worker.

mod foreground;
mod plugin_host_jni;
pub(crate) mod system_bars;

use std::path::Path;

use tauri::{AppHandle, Runtime, plugin::TauriPlugin};
use tauri_plugin_norishell_android as bridge;

pub(crate) use bridge::{DeviceKeyError, device_key_delete, device_key_unwrap, device_key_wrap};

/// The bridge plugin. Must be added to the builder so its handle exists before
/// `.setup` (Vault auto-unlock reads the Keystore during setup).
pub(crate) fn plugin<R: Runtime>() -> TauriPlugin<R> {
    bridge::init()
}

/// Called from the app `.setup` after `app_data_directory` is known.
pub(crate) fn setup(app: &AppHandle, app_data_directory: &Path) {
    crate::local_terminal_platform::configure_home(app_data_directory.join("home"));
    if !bridge::verifier_initialized() {
        bridge::log::warn(
            "platform verifier is not initialized; TLS verification will fail closed",
        );
    }
    let exit_handle = app.clone();
    let registered = bridge::registry::register_notification_action(move |action| match action {
        // Shows the exit confirmation while the WebView is alive; without it
        // Core disconnects everything and exits.
        "exitAll" => crate::lifecycle::request_application_exit(&exit_handle),
        // Swiping the task away has desktop exit semantics: disconnect all and
        // exit. The Activity and WebView may already be gone.
        "taskRemoved" => crate::lifecycle::exit_without_frontend(&exit_handle, "task_removed"),
        _ => bridge::log::warn("unknown notification action ignored"),
    });
    if registered.is_err() {
        bridge::log::warn("notification action callback already registered");
    }
}

/// Called at the end of `.setup`, after every Core service is managed: the
/// worker reads their snapshots.
pub(crate) fn start_foreground_worker(app: &AppHandle) {
    foreground::start(app.clone());
}

/// Called on `RunEvent::Exit`.
/// Every Android exit (confirmed exit, notification exit, task swipe-away,
/// Activity teardown) ends in the authorized `app.exit`, so this is the one
/// place for process-wide file cleanup.
pub(crate) fn on_exit<R: Runtime>(app: &AppHandle<R>) {
    foreground::stop(app);
    crate::content_access::remove_all_private_copies(app);
    skip_library_destructors_at_exit();
}

/// tao's Android event loop ends with `std::process::exit` on the Rust
/// event-loop thread. libc `exit()` then runs the static destructors of every
/// loaded system library while their threads keep running: libhwui's
/// `hwuiTask*` workers lock a mutex in libhwui's `.bss` after it was destroyed
/// and FORTIFY aborts the process instead of exiting.
///
/// `RunEvent::Exit` is reached only after the authorized exit path finished
/// Core cleanup, and nothing in the app relies on libc exit handlers. An
/// `atexit` handler registered now is the newest entry, so it runs before any
/// library destructor and ends the process with `_exit`. The rest of the
/// `RunEvent::Exit` handling and Rust's own stdout flush still run first.
fn skip_library_destructors_at_exit() {
    extern "C" fn terminate() {
        // SAFETY: `_exit` is async-signal-safe and never returns.
        unsafe { libc::_exit(0) }
    }
    // SAFETY: registers a plain `extern "C"` function with no captured state.
    if unsafe { libc::atexit(terminate) } != 0 {
        bridge::log::warn("exit handler registration failed");
    }
}
