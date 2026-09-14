//! JSON export for the main window only; selected paths never enter the shared filesystem scope.

use std::{io::Write as _, path::PathBuf};

use serde::Deserialize;
use tauri::{Manager as _, WebviewWindow};
use tauri_plugin_dialog::DialogExt as _;
use tokio::sync::oneshot;

const MAIN_WINDOW_LABEL: &str = "main";
const MAX_PREFERENCE_TRANSFER_BYTES: usize = 256 * 1024;
const MAX_SHORTCUT_PROFILE_BYTES: usize = 32 * 1024;
const MAX_THEME_PROFILE_BYTES: usize = 32 * 1024;
const MAX_THEME_PROFILE_OVERRIDES: usize = 32;
const MAX_THEME_PROFILE_ID_BYTES: usize = 256;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum JsonExportKind {
    Preferences,
    Shortcuts,
    Theme,
}

struct JsonExportSpec {
    file_name: &'static str,
    max_bytes: usize,
}

fn spec(kind: &JsonExportKind) -> JsonExportSpec {
    match kind {
        JsonExportKind::Preferences => JsonExportSpec {
            file_name: "norishell-preferences-v1.json",
            max_bytes: MAX_PREFERENCE_TRANSFER_BYTES,
        },
        JsonExportKind::Shortcuts => JsonExportSpec {
            file_name: "norishell-shortcuts.json",
            max_bytes: MAX_SHORTCUT_PROFILE_BYTES,
        },
        JsonExportKind::Theme => JsonExportSpec {
            file_name: "norishell-theme-profile-v1.json",
            max_bytes: MAX_THEME_PROFILE_BYTES,
        },
    }
}

fn validate_json_object(kind: &JsonExportKind, text: &str) -> Result<JsonExportSpec, String> {
    let spec = spec(kind);
    if text.len() > spec.max_bytes {
        return Err("native-json-export-too-large".to_owned());
    }
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|_| "native-json-export-invalid-json".to_owned())?;
    if !value.is_object() {
        return Err("native-json-export-invalid-json".to_owned());
    }
    if matches!(kind, JsonExportKind::Theme) && !valid_theme_profile(&value) {
        return Err("native-json-export-invalid-json".to_owned());
    }
    Ok(spec)
}

fn valid_theme_profile(value: &serde_json::Value) -> bool {
    const ROOT_KEYS: &[&str] = &["schemaVersion", "lightThemeId", "darkThemeId", "overrides"];
    const OVERRIDE_KEYS: &[&str] = &[
        "colors",
        "fontFamily",
        "fontSize",
        "radius",
        "borderWidth",
        "density",
        "shadow",
    ];
    const COLOR_KEYS: &[&str] = &[
        "bgCanvas",
        "bgSurface",
        "bgSubtle",
        "bgHover",
        "border",
        "borderStrong",
        "textPrimary",
        "textSecondary",
        "textTertiary",
        "accent",
        "accentHover",
        "onAccent",
        "accentSoft",
        "success",
        "successSoft",
        "warning",
        "warningSoft",
        "danger",
        "onDanger",
        "dangerSoft",
        "focusRing",
        "terminalPaneActiveBorder",
        "brandMarkPrimary",
        "brandMarkSecondary",
        "selection",
        "selectionText",
    ];
    let Some(profile) = value.as_object() else {
        return false;
    };
    if profile.len() != ROOT_KEYS.len()
        || !profile.keys().all(|key| ROOT_KEYS.contains(&key.as_str()))
    {
        return false;
    }
    if profile.get("schemaVersion") != Some(&serde_json::Value::from(1))
        || !bounded_theme_id(profile.get("lightThemeId"))
        || !bounded_theme_id(profile.get("darkThemeId"))
    {
        return false;
    }
    let Some(overrides) = profile
        .get("overrides")
        .and_then(serde_json::Value::as_object)
    else {
        return false;
    };
    overrides.len() <= MAX_THEME_PROFILE_OVERRIDES
        && overrides.iter().all(|(key, override_value)| {
            !key.is_empty()
                && utf16_units(key) <= MAX_THEME_PROFILE_ID_BYTES
                && valid_theme_override(override_value, OVERRIDE_KEYS, COLOR_KEYS)
        })
}

fn bounded_theme_id(value: Option<&serde_json::Value>) -> bool {
    value
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| !value.is_empty() && utf16_units(value) <= MAX_THEME_PROFILE_ID_BYTES)
}

fn utf16_units(value: &str) -> usize {
    value.encode_utf16().count()
}

fn valid_theme_override(
    value: &serde_json::Value,
    override_keys: &[&str],
    color_keys: &[&str],
) -> bool {
    let Some(override_value) = value.as_object() else {
        return false;
    };
    if override_value.len() > override_keys.len()
        || !override_value
            .keys()
            .all(|key| override_keys.contains(&key.as_str()))
    {
        return false;
    }
    if let Some(colors) = override_value.get("colors") {
        let Some(colors) = colors.as_object() else {
            return false;
        };
        if colors.len() > color_keys.len()
            || !colors.keys().all(|key| color_keys.contains(&key.as_str()))
            || !colors.values().all(valid_hex_color)
        {
            return false;
        }
    }
    optional_string_in(
        override_value.get("fontFamily"),
        &["system", "sans", "mono"],
    ) && optional_integer_in(override_value.get("fontSize"), 12, 18)
        && optional_integer_in(override_value.get("radius"), 0, 12)
        && optional_integer_in(override_value.get("borderWidth"), 1, 2)
        && optional_string_in(
            override_value.get("density"),
            &["compact", "standard", "comfortable"],
        )
        && optional_string_in(override_value.get("shadow"), &["none", "soft", "standard"])
}

fn optional_string_in(value: Option<&serde_json::Value>, allowed: &[&str]) -> bool {
    value.is_none_or(|value| value.as_str().is_some_and(|value| allowed.contains(&value)))
}

fn optional_integer_in(value: Option<&serde_json::Value>, minimum: u64, maximum: u64) -> bool {
    value.is_none_or(|value| {
        value
            .as_u64()
            .is_some_and(|value| (minimum..=maximum).contains(&value))
    })
}

fn valid_hex_color(value: &serde_json::Value) -> bool {
    value.as_str().is_some_and(|value| {
        let bytes = value.as_bytes();
        bytes.len() == 7
            && bytes[0] == b'#'
            && bytes[1..]
                .iter()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f' | b'A'..=b'F'))
    })
}

fn write_json_atomically(path: PathBuf, text: String) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| "native-json-export-write-failed".to_owned())?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| "native-json-export-write-failed".to_owned())?;
    temporary
        .write_all(text.as_bytes())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|_| "native-json-export-write-failed".to_owned())?;
    temporary
        .persist(&path)
        .map_err(|_| "native-json-export-write-failed".to_owned())?;
    Ok(())
}

/// The native save dialog writes only to the returned path; it grants no reusable or cumulative filesystem capability to the renderer.
#[tauri::command]
pub(crate) async fn native_json_export<R: tauri::Runtime>(
    window: WebviewWindow<R>,
    kind: JsonExportKind,
    text: String,
) -> Result<bool, String> {
    if window.label() != MAIN_WINDOW_LABEL {
        return Err("native-json-export-unavailable".to_owned());
    }

    let spec = validate_json_object(&kind, &text)?;

    let (sender, receiver) = oneshot::channel();
    window
        .app_handle()
        .dialog()
        .file()
        .set_parent(&window)
        .set_file_name(spec.file_name)
        .add_filter("JSON", &["json"])
        .save_file(move |path| {
            let _ = sender.send(path);
        });

    let Some(path) = receiver
        .await
        .map_err(|_| "native-json-export-unavailable".to_owned())?
    else {
        return Ok(false);
    };
    let path = path
        .into_path()
        .map_err(|_| "native-json-export-unavailable".to_owned())?;

    tauri::async_runtime::spawn_blocking(move || write_json_atomically(path, text))
        .await
        .map_err(|_| "native-json-export-write-failed".to_owned())??;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_kinds_keep_their_existing_file_names_and_byte_limits() {
        let preferences = spec(&JsonExportKind::Preferences);
        assert_eq!(preferences.file_name, "norishell-preferences-v1.json");
        assert_eq!(preferences.max_bytes, MAX_PREFERENCE_TRANSFER_BYTES);

        let shortcuts = spec(&JsonExportKind::Shortcuts);
        assert_eq!(shortcuts.file_name, "norishell-shortcuts.json");
        assert_eq!(shortcuts.max_bytes, MAX_SHORTCUT_PROFILE_BYTES);

        let theme = spec(&JsonExportKind::Theme);
        assert_eq!(theme.file_name, "norishell-theme-profile-v1.json");
        assert_eq!(theme.max_bytes, MAX_THEME_PROFILE_BYTES);
    }

    #[test]
    fn export_rejects_non_objects_and_oversized_content_before_showing_a_dialog() {
        assert!(matches!(
            validate_json_object(&JsonExportKind::Preferences, "[]"),
            Err(error) if error == "native-json-export-invalid-json"
        ));
        assert!(matches!(
            validate_json_object(
                &JsonExportKind::Shortcuts,
                &"x".repeat(MAX_SHORTCUT_PROFILE_BYTES + 1),
            ),
            Err(error) if error == "native-json-export-too-large"
        ));
    }

    #[test]
    fn atomic_write_replaces_an_existing_selected_file_only_after_the_new_content_is_ready() {
        let directory = tempfile::tempdir().expect("directory");
        let target = directory.path().join("shortcuts.json");
        std::fs::write(&target, "old").expect("old file");

        write_json_atomically(target.clone(), "{\"version\":1}".to_owned()).expect("atomic write");

        assert_eq!(
            std::fs::read_to_string(target).expect("exported file"),
            "{\"version\":1}"
        );
        assert_eq!(
            std::fs::read_dir(directory.path())
                .expect("directory")
                .count(),
            1
        );
    }

    #[test]
    fn theme_export_requires_the_bounded_profile_contract() {
        let valid = r##"{"schemaVersion":1,"lightThemeId":"builtin:light","darkThemeId":"builtin:dark","overrides":{"plugin:clear":{"colors":{"accent":"#1F5FD2"},"fontSize":14}}}"##;
        assert!(validate_json_object(&JsonExportKind::Theme, valid).is_ok());
        assert!(validate_json_object(
            &JsonExportKind::Theme,
            r#"{"schemaVersion":1,"lightThemeId":"builtin:light","darkThemeId":"builtin:dark","overrides":{},"payload":"unexpected"}"#,
        )
        .is_err());
        assert!(validate_json_object(
            &JsonExportKind::Theme,
            r##"{"schemaVersion":1,"lightThemeId":"builtin:light","darkThemeId":"builtin:dark","overrides":{"x":{"colors":{"accent":"blue"}}}}"##,
        )
        .is_err());
    }
}
