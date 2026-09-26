//! Bounded Core-owned global preferences. Local host overrides and paths are excluded.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::{RequestMeta, WireSequence};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ApplicationPreferenceGroupId {
    Application,
    Appearance,
    Interaction,
    Highlights,
    Shortcuts,
    Files,
}

impl ApplicationPreferenceGroupId {
    pub const ALL: [Self; 6] = [
        Self::Application,
        Self::Appearance,
        Self::Interaction,
        Self::Highlights,
        Self::Shortcuts,
        Self::Files,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::Appearance => "appearance",
            Self::Interaction => "interaction",
            Self::Highlights => "highlights",
            Self::Shortcuts => "shortcuts",
            Self::Files => "files",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationPreferencesGetRequest {
    pub meta: RequestMeta,
    pub group: ApplicationPreferenceGroupId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationPreferencesReplaceRequest {
    pub meta: RequestMeta,
    pub group: ApplicationPreferenceGroupId,
    pub expected_revision: Option<WireSequence>,
    #[ts(type = "unknown")]
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationPreferencesSnapshot {
    pub group: ApplicationPreferenceGroupId,
    pub revision: Option<WireSequence>,
    #[ts(type = "unknown")]
    pub value: Option<Value>,
}

impl ApplicationPreferencesSnapshot {
    pub fn validate(&self) -> Result<(), &'static str> {
        match (&self.revision, &self.value) {
            (None, None) => Ok(()),
            (Some(revision), Some(value)) if revision.get() > 0 => {
                validate_application_preference_value(self.group, value)
            }
            _ => Err("application_preferences.invalid_snapshot"),
        }
    }
}

pub fn validate_application_preference_value(
    group: ApplicationPreferenceGroupId,
    value: &Value,
) -> Result<(), &'static str> {
    // An individual group is much smaller than the existing 256 KiB transfer envelope.
    if serde_json::to_vec(value).map_or(true, |bytes| bytes.len() > 64 * 1024) {
        return Err("application_preferences.too_large");
    }
    let valid = match group {
        ApplicationPreferenceGroupId::Application => validate_application(value),
        ApplicationPreferenceGroupId::Appearance => validate_appearance(value),
        ApplicationPreferenceGroupId::Interaction => validate_interaction(value),
        ApplicationPreferenceGroupId::Highlights => validate_highlights(value),
        ApplicationPreferenceGroupId::Shortcuts => validate_shortcuts(value),
        ApplicationPreferenceGroupId::Files => validate_files(value),
    };
    valid
        .then_some(())
        .ok_or("application_preferences.invalid_input")
}

fn exact(value: &Value, keys: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == keys.len() && keys.iter().all(|key| object.contains_key(*key))
    })
}

fn optional_exact(value: &Value, required: &[&str], optional: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        required.iter().all(|key| object.contains_key(*key))
            && object
                .keys()
                .all(|key| required.contains(&key.as_str()) || optional.contains(&key.as_str()))
    })
}

fn choice(value: &Value, options: &[&str]) -> bool {
    value.as_str().is_some_and(|text| options.contains(&text))
}

fn int_between(value: &Value, min: i64, max: i64) -> bool {
    value
        .as_i64()
        .is_some_and(|number| (min..=max).contains(&number))
}

fn utf16_length(value: &str) -> usize {
    // Existing renderer preferences use JavaScript string.length (UTF-16 code units).
    value.encode_utf16().count()
}

fn hex_color(value: &Value) -> bool {
    value.as_str().is_some_and(|text| {
        text.len() == 7
            && text.starts_with('#')
            && text[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn validate_application(value: &Value) -> bool {
    exact(
        value,
        &[
            "themePreference",
            "locale",
            "uiZoom",
            "terminalStartupBehavior",
            "newTerminalBehavior",
            "singlePaneTabCloseBehavior",
        ],
    ) && choice(&value["themePreference"], &["light", "dark", "system"])
        && choice(&value["locale"], &["system", "zh-CN", "en"])
        && value["uiZoom"]
            .as_i64()
            .is_some_and(|zoom| [80, 90, 100, 110, 125, 150].contains(&zoom))
        && choice(
            &value["terminalStartupBehavior"],
            &["welcome", "restoreHistory"],
        )
        && choice(&value["newTerminalBehavior"], &["welcome", "localTerminal"])
        && choice(
            &value["singlePaneTabCloseBehavior"],
            &["confirm", "closeDirectly"],
        )
}

const TERMINAL_COLORS: &[&str] = &[
    "background",
    "foreground",
    "muted",
    "cursor",
    "selection",
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "brightBlack",
    "brightRed",
    "brightGreen",
    "brightYellow",
    "brightBlue",
    "brightMagenta",
    "brightCyan",
    "brightWhite",
];

fn validate_palette(value: &Value) -> bool {
    exact(value, TERMINAL_COLORS) && TERMINAL_COLORS.iter().all(|key| hex_color(&value[*key]))
}

fn validate_appearance(value: &Value) -> bool {
    let required = [
        "terminalThemeMode",
        "terminalFontFamily",
        "terminalFontSize",
        "terminalFontWeight",
        "terminalBoldFontWeight",
        "terminalLineHeight",
        "terminalLetterSpacing",
        "terminalCursorStyle",
        "terminalCursorBlink",
        "customTerminalPalette",
        "customTerminalPaletteName",
    ];
    if !optional_exact(value, &required, &["appTheme"]) {
        return false;
    }
    let font_family = value["terminalFontFamily"].as_str().unwrap_or("");
    let palette_name = value["customTerminalPaletteName"].as_str().unwrap_or("");
    let line_height = value["terminalLineHeight"].as_f64().unwrap_or(-1.0);
    choice(
        &value["terminalThemeMode"],
        &[
            "follow-app",
            "custom",
            "norishell-light",
            "norishell-dark",
            "solarized-light",
            "solarized-dark",
            "nord",
            "gruvbox-dark",
        ],
    ) && utf16_length(font_family) <= 80
        && font_family == font_family.trim()
        && !font_family.contains("  ")
        && !font_family
            .chars()
            .any(|character| character.is_control() || ",;{}()'\"\\".contains(character))
        && int_between(&value["terminalFontSize"], 10, 28)
        && ["terminalFontWeight", "terminalBoldFontWeight"]
            .iter()
            .all(|key| {
                value[*key]
                    .as_i64()
                    .is_some_and(|weight| (300..=900).contains(&weight) && weight % 100 == 0)
            })
        && (1.0..=2.0).contains(&line_height)
        && (line_height * 10.0).fract().abs() < 0.000_001
        && int_between(&value["terminalLetterSpacing"], -2, 4)
        && choice(
            &value["terminalCursorStyle"],
            &["block", "bar", "underline"],
        )
        && value["terminalCursorBlink"].is_boolean()
        && validate_palette(&value["customTerminalPalette"])
        && utf16_length(palette_name) <= 48
        && palette_name == palette_name.trim()
        && value.get("appTheme").is_none_or(validate_app_theme)
}

const APP_THEME_COLORS: &[&str] = &[
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

fn validate_app_theme(value: &Value) -> bool {
    if !exact(
        value,
        &["schemaVersion", "lightThemeId", "darkThemeId", "overrides"],
    ) || value["schemaVersion"] != 1
        || serde_json::to_vec(value).map_or(true, |bytes| bytes.len() > 32 * 1024)
    {
        return false;
    }
    let valid_id = |key: &str| {
        value[key]
            .as_str()
            .is_some_and(|id| !id.is_empty() && utf16_length(id) <= 256)
    };
    let Some(overrides) = value["overrides"].as_object() else {
        return false;
    };
    valid_id("lightThemeId")
        && valid_id("darkThemeId")
        && overrides.len() <= 32
        && overrides.iter().all(|(key, override_value)| {
            !key.is_empty()
                && utf16_length(key) <= 256
                && optional_exact(
                    override_value,
                    &[],
                    &[
                        "colors",
                        "fontFamily",
                        "fontSize",
                        "radius",
                        "borderWidth",
                        "density",
                        "shadow",
                    ],
                )
                && override_value.get("colors").is_none_or(|colors| {
                    colors.as_object().is_some_and(|colors| {
                        colors.iter().all(|(name, color)| {
                            APP_THEME_COLORS.contains(&name.as_str()) && hex_color(color)
                        })
                    })
                })
                && override_value
                    .get("fontFamily")
                    .is_none_or(|v| choice(v, &["system", "sans", "mono"]))
                && override_value
                    .get("fontSize")
                    .is_none_or(|v| int_between(v, 12, 18))
                && override_value
                    .get("radius")
                    .is_none_or(|v| int_between(v, 0, 12))
                && override_value
                    .get("borderWidth")
                    .is_none_or(|v| int_between(v, 1, 2))
                && override_value
                    .get("density")
                    .is_none_or(|v| choice(v, &["compact", "standard", "comfortable"]))
                && override_value
                    .get("shadow")
                    .is_none_or(|v| choice(v, &["none", "soft", "standard"]))
        })
}

fn validate_interaction(value: &Value) -> bool {
    let inner = &value["interaction"];
    exact(value, &["interaction", "pasteWarning"])
        && choice(&value["pasteWarning"], &["always", "multiline", "never"])
        && exact(
            inner,
            &[
                "scrollback",
                "scrollSensitivity",
                "smoothScrollDuration",
                "doubleClickSelection",
                "copyOnSelect",
                "rightClickBehavior",
                "optionAsMetaLeft",
                "optionAsMetaRight",
                "backspaceMode",
                "bellMode",
                "linksEnabled",
                "sshReconnectOnInput",
            ],
        )
        && int_between(&inner["scrollback"], 1_000, 100_000)
        && int_between(&inner["scrollSensitivity"], 1, 10)
        && [0, 100, 200].contains(&inner["smoothScrollDuration"].as_i64().unwrap_or(-1))
        && choice(&inner["doubleClickSelection"], &["word", "path", "address"])
        && choice(&inner["rightClickBehavior"], &["menu", "paste"])
        && choice(&inner["backspaceMode"], &["del", "bs"])
        && choice(&inner["bellMode"], &["off", "visual", "sound"])
        && [
            "copyOnSelect",
            "optionAsMetaLeft",
            "optionAsMetaRight",
            "linksEnabled",
            "sshReconnectOnInput",
        ]
        .iter()
        .all(|key| inner[*key].is_boolean())
}

// The renderer validates ECMAScript syntax before execution. Rust's regex
// dialect would reject valid saved JS lookarounds and backreferences.
fn validate_highlights(value: &Value) -> bool {
    let Some(rules) = value["rules"].as_array() else {
        return false;
    };
    let mut ids = std::collections::HashSet::new();
    exact(value, &["enabled", "rules"])
        && value["enabled"].is_boolean()
        && rules.len() <= 32
        && rules.iter().all(|rule| {
            let id = rule["id"].as_str().unwrap_or("");
            let label = rule["label"].as_str().unwrap_or("");
            let pattern = rule["pattern"].as_str().unwrap_or("");
            exact(
                rule,
                &[
                    "id",
                    "label",
                    "pattern",
                    "mode",
                    "caseSensitive",
                    "foreground",
                    "background",
                    "enabled",
                ],
            ) && (1..=80).contains(&id.len())
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
                && ids.insert(id.to_owned())
                && !label.trim().is_empty()
                && utf16_length(label) <= 48
                && !pattern.is_empty()
                && utf16_length(pattern) <= 256
                && !label.chars().chain(pattern.chars()).any(char::is_control)
                && choice(&rule["mode"], &["literal", "regex"])
                && rule["caseSensitive"].is_boolean()
                && rule["enabled"].is_boolean()
                && hex_color(&rule["foreground"])
                && hex_color(&rule["background"])
        })
}

const SHORTCUT_IDS: &[&str] = &[
    "navigation.overview",
    "navigation.terminal",
    "navigation.hosts",
    "navigation.sftp",
    "navigation.tunnels",
    "navigation.plugins",
    "navigation.settings",
    "navigation.known-hosts",
    "navigation.ssh-identities",
    "workspace.tab.1",
    "workspace.tab.2",
    "workspace.tab.3",
    "workspace.tab.4",
    "workspace.tab.5",
    "workspace.tab.6",
    "workspace.tab.7",
    "workspace.tab.8",
    "workspace.tab.9",
    "workspace.new",
    "workspace.close",
    "workspace.next",
    "workspace.previous",
    "workspace.new-local",
    "terminal.split-right",
    "terminal.split-down",
    "terminal.focus-next-pane",
    "terminal.focus-previous-pane",
    "terminal.search",
    "terminal.copy",
    "terminal.paste",
    "terminal.clear",
    "terminal.toggle-quick-commands",
    "terminal.history-suggestions",
    "terminal.reconnect",
    "terminal.disconnect",
    "terminal.close-pane",
];

fn valid_shortcut_binding(value: &Value, platform: &str, app_scope: bool) -> bool {
    let Some(binding) = value.as_str() else {
        return value.is_null();
    };
    if binding.len() > 96 {
        return false;
    }
    let parts: Vec<_> = binding.split('+').collect();
    let Some(code) = parts.last().copied() else {
        return false;
    };
    let modifiers = &parts[..parts.len().saturating_sub(1)];
    if modifiers.is_empty()
        || modifiers.len() > 4
        || !["Meta", "Ctrl", "Alt", "Shift"]
            .iter()
            .filter(|modifier| modifiers.contains(modifier))
            .eq(modifiers.iter())
        || (platform == "windows" && modifiers.contains(&"Meta"))
        || (app_scope && !modifiers.contains(&if platform == "macos" { "Meta" } else { "Ctrl" }))
    {
        return false;
    }
    let code_valid = (code.len() == 4
        && code.starts_with("Key")
        && code.as_bytes()[3].is_ascii_uppercase())
        || (code.len() == 6 && code.starts_with("Digit") && code.as_bytes()[5].is_ascii_digit())
        || (code.starts_with('F') && code[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)))
        || [
            "ArrowUp",
            "ArrowDown",
            "ArrowLeft",
            "ArrowRight",
            "Enter",
            "Escape",
            "Space",
            "Tab",
            "Backspace",
            "Delete",
            "Home",
            "End",
            "PageUp",
            "PageDown",
            "BracketLeft",
            "BracketRight",
            "Comma",
            "Period",
            "Slash",
            "Minus",
            "Equal",
            "Quote",
            "Backquote",
            "Semicolon",
        ]
        .contains(&code);
    if !code_valid {
        return false;
    }
    let has = |modifier: &str| modifiers.contains(&modifier);
    if platform == "windows" {
        if has("Ctrl")
            && !has("Alt")
            && !has("Shift")
            && ["KeyC", "KeyV", "KeyX", "KeyZ", "KeyA", "KeyY", "KeyM"].contains(&code)
        {
            return false;
        }
        if (has("Alt") && ["F4", "Tab", "Escape"].contains(&code))
            || (has("Ctrl") && has("Alt") && code == "Delete")
            || (has("Ctrl") && has("Shift") && code == "Escape")
        {
            return false;
        }
    } else {
        if (has("Meta")
            && !has("Ctrl")
            && !has("Alt")
            && !has("Shift")
            && [
                "KeyQ", "KeyH", "KeyM", "KeyC", "KeyV", "KeyX", "KeyZ", "KeyA", "Tab", "Space",
            ]
            .contains(&code))
            || (has("Meta") && has("Shift") && !has("Alt") && !has("Ctrl") && code == "KeyZ")
            || (has("Meta") && has("Alt") && code == "Escape")
            || (has("Ctrl") && has("Meta") && ["KeyQ", "KeyF"].contains(&code))
            || (has("Meta") && has("Alt") && !has("Ctrl") && !has("Shift") && code == "KeyH")
        {
            return false;
        }
    }
    true
}

fn validate_shortcuts(value: &Value) -> bool {
    if !exact(value, &["version", "bindings"])
        || value["version"] != 1
        || !exact(&value["bindings"], &["macos", "windows"])
    {
        return false;
    }
    ["macos", "windows"].iter().all(|platform| {
        let bindings = &value["bindings"][*platform];
        let mut seen = std::collections::HashSet::new();
        exact(bindings, SHORTCUT_IDS)
            && SHORTCUT_IDS.iter().all(|id| {
                let binding = &bindings[*id];
                valid_shortcut_binding(binding, platform, !id.starts_with("terminal."))
                    && binding
                        .as_str()
                        .is_none_or(|text| seen.insert(text.to_owned()))
            })
    })
}

fn validate_files(value: &Value) -> bool {
    let browser = &value["browser"];
    exact(value, &["browser", "rememberLastDirectory"])
        && value["rememberLastDirectory"].is_boolean()
        && exact(browser, &["showHidden", "foldersFirst", "sort"])
        && browser["showHidden"].is_boolean()
        && browser["foldersFirst"].is_boolean()
        && choice(&browser["sort"], &["name", "size", "modified"])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn each_group_rejects_unregistered_or_local_only_fields() {
        let application = json!({
            "themePreference":"system", "locale":"en", "uiZoom":125,
            "terminalStartupBehavior":"welcome", "newTerminalBehavior":"localTerminal",
            "singlePaneTabCloseBehavior":"confirm"
        });
        assert!(
            validate_application_preference_value(
                ApplicationPreferenceGroupId::Application,
                &application
            )
            .is_ok()
        );
        let mut invalid = application.clone();
        invalid["hostId"] = json!("private-host");
        assert!(
            validate_application_preference_value(
                ApplicationPreferenceGroupId::Application,
                &invalid
            )
            .is_err()
        );

        let files = json!({"browser":{"showHidden":true,"foldersFirst":true,"sort":"name"},"rememberLastDirectory":false});
        assert!(
            validate_application_preference_value(ApplicationPreferenceGroupId::Files, &files)
                .is_ok()
        );
        let mut invalid = files.clone();
        invalid["directories"] = json!({"local":"/secret","remote":[]});
        assert!(
            validate_application_preference_value(ApplicationPreferenceGroupId::Files, &invalid)
                .is_err()
        );

        let highlights = json!({"enabled":true,"rules":[{
            "id":"error", "label":"ERROR", "pattern":"[A-Z]+", "mode":"regex",
            "caseSensitive":false, "foreground":"#ffffff", "background":"#a82d39", "enabled":true
        }]});
        assert!(
            validate_application_preference_value(
                ApplicationPreferenceGroupId::Highlights,
                &highlights
            )
            .is_ok()
        );
        let mut invalid = highlights.clone();
        invalid["rules"][0]["pattern"] = json!("\u{0000}");
        assert!(
            validate_application_preference_value(
                ApplicationPreferenceGroupId::Highlights,
                &invalid
            )
            .is_err()
        );

        let interaction = json!({"pasteWarning":"multiline","interaction":{
            "scrollback":5000,"scrollSensitivity":1,"smoothScrollDuration":0,
            "doubleClickSelection":"word","copyOnSelect":false,"rightClickBehavior":"menu",
            "optionAsMetaLeft":false,"optionAsMetaRight":false,"backspaceMode":"del",
            "bellMode":"off","linksEnabled":true,"sshReconnectOnInput":true
        }});
        assert!(
            validate_application_preference_value(
                ApplicationPreferenceGroupId::Interaction,
                &interaction
            )
            .is_ok()
        );
        let mut invalid = interaction.clone();
        invalid["interaction"]["hostKeyboard"] = json!({});
        assert!(
            validate_application_preference_value(
                ApplicationPreferenceGroupId::Interaction,
                &invalid
            )
            .is_err()
        );
    }

    #[test]
    fn appearance_and_shortcuts_enforce_field_domains() {
        let mut palette = serde_json::Map::new();
        for color in TERMINAL_COLORS {
            palette.insert((*color).to_owned(), json!("#abcdef"));
        }
        let appearance = json!({
            "terminalThemeMode":"follow-app", "terminalFontFamily":"", "terminalFontSize":14,
            "terminalFontWeight":400, "terminalBoldFontWeight":700, "terminalLineHeight":1.2,
            "terminalLetterSpacing":0, "terminalCursorStyle":"block", "terminalCursorBlink":true,
            "customTerminalPalette":palette, "customTerminalPaletteName":"",
            "appTheme":{"schemaVersion":1,"lightThemeId":"builtin:light","darkThemeId":"builtin:dark","overrides":{}}
        });
        assert!(
            validate_application_preference_value(
                ApplicationPreferenceGroupId::Appearance,
                &appearance
            )
            .is_ok()
        );
        let mut existing_unicode = appearance.clone();
        existing_unicode["terminalFontFamily"] = json!("思源等宽".repeat(16));
        existing_unicode["customTerminalPaletteName"] = json!("终端主题".repeat(10));
        existing_unicode["appTheme"]["lightThemeId"] = json!("浅色".repeat(80));
        existing_unicode["appTheme"]["overrides"]["深色".repeat(80)] = json!({});
        assert!(
            validate_application_preference_value(
                ApplicationPreferenceGroupId::Appearance,
                &existing_unicode
            )
            .is_ok()
        );
        let mut invalid = appearance.clone();
        invalid["appTheme"]["overrides"]["a"]["colors"]["evil"] = json!("#abcdef");
        assert!(
            validate_application_preference_value(
                ApplicationPreferenceGroupId::Appearance,
                &invalid
            )
            .is_err()
        );

        let mut empty_bindings = serde_json::Map::new();
        for id in SHORTCUT_IDS {
            empty_bindings.insert((*id).to_owned(), Value::Null);
        }
        let shortcuts =
            json!({"version":1,"bindings":{"macos":empty_bindings,"windows":empty_bindings}});
        assert!(
            validate_application_preference_value(
                ApplicationPreferenceGroupId::Shortcuts,
                &shortcuts
            )
            .is_ok()
        );
        let mut invalid = shortcuts.clone();
        invalid["bindings"]["macos"]["workspace.new"] = json!("Meta+KeyQ");
        assert!(
            validate_application_preference_value(
                ApplicationPreferenceGroupId::Shortcuts,
                &invalid
            )
            .is_err()
        );
    }
}
