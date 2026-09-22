#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(exit_code) = norishell_desktop::run_plugin_host_if_requested() {
        std::process::exit(exit_code);
    }
    norishell_desktop::run();
}
