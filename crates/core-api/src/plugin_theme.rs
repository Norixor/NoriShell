use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{PluginId, RequestMeta};

pub const MAX_THEME_DEFINITION_BYTES: usize = 32 * 1024;
pub const MAX_THEME_PACKAGE_ENTRIES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ThemeAppearance {
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ThemeFontFamily {
    System,
    Sans,
    Mono,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ThemeDensity {
    Compact,
    Standard,
    Comfortable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ThemeShadow {
    None,
    Soft,
    Standard,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThemeName {
    #[serde(rename = "zhCN")]
    #[ts(rename = "zhCN")]
    pub zh_cn: String,
    pub en: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThemeColors {
    pub bg_canvas: String,
    pub bg_surface: String,
    pub bg_subtle: String,
    pub bg_hover: String,
    pub border: String,
    pub border_strong: String,
    pub text_primary: String,
    pub text_secondary: String,
    pub text_tertiary: String,
    pub accent: String,
    pub accent_hover: String,
    pub on_accent: String,
    pub accent_soft: String,
    pub success: String,
    pub success_soft: String,
    pub warning: String,
    pub warning_soft: String,
    pub danger: String,
    pub on_danger: String,
    pub danger_soft: String,
    pub focus_ring: String,
    pub terminal_pane_active_border: String,
    pub brand_mark_primary: String,
    pub brand_mark_secondary: String,
    pub selection: String,
    pub selection_text: String,
}

impl ThemeColors {
    fn values(&self) -> [&str; 26] {
        [
            &self.bg_canvas,
            &self.bg_surface,
            &self.bg_subtle,
            &self.bg_hover,
            &self.border,
            &self.border_strong,
            &self.text_primary,
            &self.text_secondary,
            &self.text_tertiary,
            &self.accent,
            &self.accent_hover,
            &self.on_accent,
            &self.accent_soft,
            &self.success,
            &self.success_soft,
            &self.warning,
            &self.warning_soft,
            &self.danger,
            &self.on_danger,
            &self.danger_soft,
            &self.focus_ring,
            &self.terminal_pane_active_border,
            &self.brand_mark_primary,
            &self.brand_mark_secondary,
            &self.selection,
            &self.selection_text,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThemeTerminalPalette {
    pub background: String,
    pub foreground: String,
    pub muted: String,
    pub cursor: String,
    pub selection: String,
    pub black: String,
    pub red: String,
    pub green: String,
    pub yellow: String,
    pub blue: String,
    pub magenta: String,
    pub cyan: String,
    pub white: String,
    pub bright_black: String,
    pub bright_red: String,
    pub bright_green: String,
    pub bright_yellow: String,
    pub bright_blue: String,
    pub bright_magenta: String,
    pub bright_cyan: String,
    pub bright_white: String,
}

impl ThemeTerminalPalette {
    fn values(&self) -> [&str; 21] {
        [
            &self.background,
            &self.foreground,
            &self.muted,
            &self.cursor,
            &self.selection,
            &self.black,
            &self.red,
            &self.green,
            &self.yellow,
            &self.blue,
            &self.magenta,
            &self.cyan,
            &self.white,
            &self.bright_black,
            &self.bright_red,
            &self.bright_green,
            &self.bright_yellow,
            &self.bright_blue,
            &self.bright_magenta,
            &self.bright_cyan,
            &self.bright_white,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThemeDefinition {
    pub schema_version: u8,
    pub id: String,
    pub name: ThemeName,
    pub appearance: ThemeAppearance,
    pub colors: ThemeColors,
    pub font_family: ThemeFontFamily,
    pub font_size: u8,
    pub radius: u8,
    pub border_width: u8,
    pub density: ThemeDensity,
    pub shadow: ThemeShadow,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub terminal_palette: Option<ThemeTerminalPalette>,
}

impl ThemeDefinition {
    pub fn parse_json(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.is_empty() || bytes.len() > MAX_THEME_DEFINITION_BYTES {
            return Err("theme definition must be a non-empty bounded JSON document");
        }
        let definition: Self = serde_json::from_slice(bytes)
            .map_err(|_| "theme definition must match the supported JSON schema")?;
        definition.validate()?;
        Ok(definition)
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != 1 {
            return Err("theme schema_version must be 1");
        }
        if !valid_slug(&self.id) {
            return Err("theme id must be a bounded lowercase slug");
        }
        if !valid_theme_name(&self.name.zh_cn) || !valid_theme_name(&self.name.en) {
            return Err("theme names must be non-empty bounded display text");
        }
        if !(12..=18).contains(&self.font_size) {
            return Err("theme font_size must be between 12 and 18");
        }
        if self.radius > 12 {
            return Err("theme radius must be between 0 and 12");
        }
        if !(1..=2).contains(&self.border_width) {
            return Err("theme border_width must be between 1 and 2");
        }
        if self.colors.values().iter().any(|color| !valid_color(color)) {
            return Err("theme colors must be #RRGGBB values");
        }
        if self
            .terminal_palette
            .as_ref()
            .is_some_and(|palette| palette.values().iter().any(|color| !valid_color(color)))
        {
            return Err("theme terminal palette colors must be #RRGGBB values");
        }

        let colors = &self.colors;
        for foreground in [&colors.text_primary, &colors.text_secondary] {
            for background in [&colors.bg_canvas, &colors.bg_surface, &colors.bg_subtle] {
                if contrast_ratio(foreground, background) < 4.5 {
                    return Err("theme text colors do not meet the required contrast");
                }
            }
        }
        if contrast_ratio(&colors.on_accent, &colors.accent) < 4.5
            || contrast_ratio(&colors.on_accent, &colors.accent_hover) < 4.5
            || contrast_ratio(&colors.on_danger, &colors.danger) < 4.5
        {
            return Err("theme action colors do not meet the required contrast");
        }
        if contrast_ratio(&colors.focus_ring, &colors.bg_canvas) < 3.0 {
            return Err("theme focus ring does not meet the required contrast");
        }
        if contrast_ratio(&colors.selection_text, &colors.selection) < 4.5 {
            return Err("theme selection colors do not meet the required contrast");
        }
        for background in [
            &colors.accent_soft,
            &colors.success_soft,
            &colors.warning_soft,
            &colors.danger_soft,
        ] {
            if contrast_ratio(&colors.text_primary, background) < 4.5 {
                return Err("theme soft colors do not meet the required contrast");
            }
        }
        for (foreground, background) in [
            (&colors.success, &colors.success_soft),
            (&colors.warning, &colors.warning_soft),
            (&colors.danger, &colors.danger_soft),
        ] {
            if contrast_ratio(foreground, background) < 3.0 {
                return Err("theme status colors do not meet the required contrast");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ThemePackageEntry {
    pub plugin_id: PluginId,
    pub package_hash: String,
    pub version: String,
    pub enabled: bool,
    pub definition: ThemeDefinition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginThemeListRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginThemeListResponse {
    pub themes: Vec<ThemePackageEntry>,
}

fn valid_slug(value: &str) -> bool {
    if value.is_empty() || value.len() > 64 || !value.is_ascii() {
        return false;
    }
    let mut previous_separator = false;
    for (index, byte) in value.bytes().enumerate() {
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            previous_separator = false;
        } else if matches!(byte, b'.' | b'_' | b'-') {
            if index == 0 || previous_separator {
                return false;
            }
            previous_separator = true;
        } else {
            return false;
        }
    }
    !previous_separator
}

fn valid_theme_name(value: &str) -> bool {
    !value.trim().is_empty()
        && value.encode_utf16().count() <= 80
        && !value.chars().any(char::is_control)
}

fn valid_color(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 7
        && bytes[0] == b'#'
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f' | b'A'..=b'F'))
}

fn contrast_ratio(left: &str, right: &str) -> f64 {
    let left = relative_luminance(left);
    let right = relative_luminance(right);
    (left.max(right) + 0.05) / (left.min(right) + 0.05)
}

fn relative_luminance(value: &str) -> f64 {
    let bytes = value.as_bytes();
    let channel = |offset| {
        let high = hex_nibble(bytes[offset]).unwrap_or_default();
        let low = hex_nibble(bytes[offset + 1]).unwrap_or_default();
        let value = f64::from((high << 4) | low) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5)
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::ThemeDefinition;

    const VALID_THEME: &str = r##"{
      "schemaVersion":1,
      "id":"clear",
      "name":{"zhCN":"澄明","en":"Clear"},
      "appearance":"light",
      "colors":{
        "bgCanvas":"#F7F8FA","bgSurface":"#FFFFFF","bgSubtle":"#EEF1F5","bgHover":"#E7ECF3",
        "border":"#C7CFDA","borderStrong":"#8794A6","textPrimary":"#171A21","textSecondary":"#3C4555","textTertiary":"#647184",
        "accent":"#1F5FD2","accentHover":"#174CA8","onAccent":"#FFFFFF","accentSoft":"#DCE9FF",
        "success":"#16794B","successSoft":"#DDF5E7","warning":"#8A5800","warningSoft":"#FFF0C7",
        "danger":"#C62835","onDanger":"#FFFFFF","dangerSoft":"#FDE1E3","focusRing":"#1F5FD2",
        "terminalPaneActiveBorder":"#1F5FD2","brandMarkPrimary":"#1F5FD2","brandMarkSecondary":"#6D92E8","selection":"#B9D1FF","selectionText":"#171A21"
      },
      "fontFamily":"system","fontSize":14,"radius":8,"borderWidth":1,"density":"standard","shadow":"soft"
    }"##;

    #[test]
    fn parses_a_complete_bounded_theme_definition() {
        let definition = ThemeDefinition::parse_json(VALID_THEME.as_bytes()).expect("valid theme");
        assert_eq!(definition.id, "clear");
    }

    #[test]
    fn rejects_unknown_fields_and_insufficient_contrast() {
        assert!(
            ThemeDefinition::parse_json(
                VALID_THEME
                    .replace(
                        "\"shadow\":\"soft\"",
                        "\"shadow\":\"soft\",\"url\":\"https://example.test\"",
                    )
                    .as_bytes()
            )
            .is_err()
        );
        assert!(
            ThemeDefinition::parse_json(
                VALID_THEME
                    .replace(
                        "\"textSecondary\":\"#3C4555\"",
                        "\"textSecondary\":\"#EEF1F5\"",
                    )
                    .as_bytes()
            )
            .is_err()
        );
    }

    #[test]
    fn matches_the_frontend_slug_and_utf16_name_bounds() {
        assert!(
            ThemeDefinition::parse_json(
                VALID_THEME
                    .replace("\"clear\"", "\"clear.theme_1\"")
                    .as_bytes()
            )
            .is_ok()
        );
        assert!(
            ThemeDefinition::parse_json(
                VALID_THEME
                    .replace("\"clear\"", "\"clear..theme\"")
                    .as_bytes()
            )
            .is_err()
        );
        let eighty_one_astral = "😀".repeat(41);
        assert!(
            ThemeDefinition::parse_json(
                VALID_THEME
                    .replace("\"Clear\"", &eighty_one_astral)
                    .as_bytes()
            )
            .is_err()
        );
    }
}
