// The bridge exposes no JavaScript commands: every Kotlin command is reachable
// only from Rust through `PluginHandle::run_mobile_plugin`.
const COMMANDS: &[&str] = &[];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .build();
}
