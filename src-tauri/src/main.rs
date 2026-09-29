#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// The AppImage GTK hook forces `GDK_BACKEND=x11`, which the Linux window shape (transparent
/// rounded corners, native resize edges) is not built for. Undo it when a Wayland session exists.
#[cfg(target_os = "linux")]
fn prefer_wayland_in_appimage() {
    let in_appimage =
        std::env::var_os("APPDIR").is_some() && std::env::var_os("APPIMAGE").is_some();
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty());
    if in_appimage && wayland && std::env::var("GDK_BACKEND").as_deref() == Ok("x11") {
        // SAFETY: called first thing in main, before any other thread exists.
        unsafe { std::env::remove_var("GDK_BACKEND") };
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    prefer_wayland_in_appimage();
    if let Some(exit_code) = norishell_desktop::run_plugin_host_if_requested() {
        std::process::exit(exit_code);
    }
    norishell_desktop::run();
}
