//! Opt-in timing of native Tab WebView startup. Each mark holds only a stage name,
//! the view role, a non-identifying trace hash and a timestamp; enabled in debug
//! builds or when `NORISHELL_TAB_BOOT_TRACE=1`, otherwise every mark is refused.

use std::{io::Write, sync::OnceLock};

use serde::Deserialize;
use tauri::{AppHandle, Manager, Runtime};

const ENV: &str = "NORISHELL_TAB_BOOT_TRACE";
const LOG_FILE: &str = "tab-boot-trace.log";

fn env_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var(ENV).is_ok_and(|value| value == "1"))
}

fn enabled() -> bool {
    cfg!(debug_assertions) || env_enabled()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TabBootMark {
    trace: String,
    view: String,
    stage: String,
    at_ms: f64,
}

impl TabBootMark {
    fn valid(&self) -> bool {
        self.trace.len() == 8
            && self.trace.bytes().all(|byte| byte.is_ascii_hexdigit())
            && matches!(self.view.as_str(), "shell" | "tab")
            && !self.stage.is_empty()
            && self.stage.len() <= 40
            && self
                .stage
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
            && self.at_ms.is_finite()
            && (0.0..1e15).contains(&self.at_ms)
    }

    fn line(&self) -> String {
        // Every field was validated to a JSON-safe alphabet above.
        format!(
            "{{\"trace\":\"{}\",\"view\":\"{}\",\"stage\":\"{}\",\"atMs\":{:.1}}}",
            self.trace, self.view, self.stage, self.at_ms
        )
    }
}

#[tauri::command]
pub(crate) fn tab_boot_trace_enabled() -> bool {
    enabled()
}

#[tauri::command]
pub(crate) fn tab_boot_trace<R: Runtime>(
    app: AppHandle<R>,
    mark: TabBootMark,
) -> Result<(), String> {
    if !enabled() {
        return Err("tab_boot_trace.disabled".into());
    }
    if !mark.valid() {
        return Err("tab_boot_trace.invalid_mark".into());
    }
    let line = mark.line();
    eprintln!("[tab-boot] {line}");
    // A release build has no console on Windows; the explicit switch also keeps a file.
    if env_enabled()
        && let Ok(dir) = app.path().app_log_dir()
        && std::fs::create_dir_all(&dir).is_ok()
        && let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(LOG_FILE))
    {
        let _ = writeln!(file, "[tab-boot] {line}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(stage: &str, trace: &str) -> TabBootMark {
        TabBootMark {
            trace: trace.into(),
            view: "tab".into(),
            stage: stage.into(),
            at_ms: 1_700_000_000_000.5,
        }
    }

    #[test]
    fn marks_carry_only_bounded_identifiers() {
        assert!(mark("bootstrap_received", "0a1b2c3d").valid());
        assert_eq!(
            mark("ready_sent", "0a1b2c3d").line(),
            r#"{"trace":"0a1b2c3d","view":"tab","stage":"ready_sent","atMs":1700000000000.5}"#
        );
        assert!(!mark("host=prod.example", "0a1b2c3d").valid());
        assert!(!mark("ready", "not-hex!").valid());
        assert!(!mark("\"inject\"", "0a1b2c3d").valid());
    }
}
