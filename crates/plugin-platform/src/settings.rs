//! Validation and migration for the fixed `assets/settings.json` package asset.

use std::collections::{BTreeMap, BTreeSet};

use norishell_core_api::{
    PLUGIN_SETTINGS_SCHEMA_VERSION, PluginSettingField, PluginSettingLabel, PluginSettingValue,
    PluginSettingsSchema, PluginSettingsValues,
};
use sha2::{Digest, Sha256};

use crate::{PluginPlatformError, Result, package::lower_hex};

pub const PLUGIN_SETTINGS_ASSET_PATH: &str = "assets/settings.json";
pub const MAX_PLUGIN_SETTINGS_SCHEMA_BYTES: usize = 32 * 1024;
pub const MAX_PLUGIN_SETTINGS_VALUES_BYTES: usize = 16 * 1024;
const MAX_PLUGIN_SETTINGS_FIELDS: usize = 32;
const MAX_SETTING_KEY_BYTES: usize = 80;
const MAX_SETTING_LABEL_BYTES: usize = 512;
const MAX_SELECT_OPTIONS: usize = 256;
const MAX_SELECT_VALUE_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq)]
pub struct InspectedPluginSettings {
    pub schema: PluginSettingsSchema,
    pub schema_json: String,
    pub schema_sha256: String,
    pub default_values: PluginSettingsValues,
    pub default_values_json: String,
}

pub fn inspect_plugin_settings(bytes: &[u8]) -> Result<InspectedPluginSettings> {
    if bytes.is_empty() || bytes.len() > MAX_PLUGIN_SETTINGS_SCHEMA_BYTES {
        return Err(PluginPlatformError::InvalidSettingsSchema);
    }
    let schema: PluginSettingsSchema =
        serde_json::from_slice(bytes).map_err(|_| PluginPlatformError::InvalidSettingsSchema)?;
    validate_settings_schema(&schema)?;
    let schema_json =
        serde_json::to_string(&schema).map_err(|_| PluginPlatformError::InvalidSettingsSchema)?;
    if schema_json.len() > MAX_PLUGIN_SETTINGS_SCHEMA_BYTES {
        return Err(PluginPlatformError::InvalidSettingsSchema);
    }
    let default_values = default_settings_values(&schema);
    let default_values_json = settings_values_json(&schema, &default_values)?;
    let schema_sha256 = lower_hex(&Sha256::digest(schema_json.as_bytes()));
    Ok(InspectedPluginSettings {
        schema,
        schema_json,
        schema_sha256,
        default_values,
        default_values_json,
    })
}

pub fn validate_settings_schema(schema: &PluginSettingsSchema) -> Result<()> {
    if schema.schema_version != PLUGIN_SETTINGS_SCHEMA_VERSION
        || schema.fields.is_empty()
        || schema.fields.len() > MAX_PLUGIN_SETTINGS_FIELDS
    {
        return Err(PluginPlatformError::InvalidSettingsSchema);
    }
    let mut keys = BTreeSet::new();
    let mut kinds = BTreeMap::new();
    for field in &schema.fields {
        let key = field.key();
        if !valid_setting_key(key)
            || norishell_core_api::sensitive_plugin_setting_key(key)
            || !keys.insert(key.to_owned())
        {
            return Err(PluginPlatformError::InvalidSettingsSchema);
        }
        match field {
            PluginSettingField::Boolean { label, .. } => {
                validate_label(label)?;
                kinds.insert(key, SettingKind::Boolean);
            }
            PluginSettingField::Number {
                label,
                default,
                min,
                max,
                ..
            } => {
                validate_label(label)?;
                if !default.is_finite()
                    || min.is_some_and(|value| !value.is_finite())
                    || max.is_some_and(|value| !value.is_finite())
                    || min.zip(*max).is_some_and(|(min, max)| min > max)
                    || min.is_some_and(|min| *default < min)
                    || max.is_some_and(|max| *default > max)
                {
                    return Err(PluginPlatformError::InvalidSettingsSchema);
                }
                kinds.insert(key, SettingKind::Number);
            }
            PluginSettingField::String {
                label,
                default,
                max_length,
                ..
            } => {
                validate_label(label)?;
                if !(1..=2048).contains(max_length)
                    || default.chars().count() > usize::from(*max_length)
                    || invalid_text(default)
                {
                    return Err(PluginPlatformError::InvalidSettingsSchema);
                }
                kinds.insert(key, SettingKind::String);
            }
            PluginSettingField::Select {
                label,
                default,
                options,
                ..
            } => {
                validate_label(label)?;
                if options.is_empty() || options.len() > MAX_SELECT_OPTIONS {
                    return Err(PluginPlatformError::InvalidSettingsSchema);
                }
                let mut values = BTreeSet::new();
                for option in options {
                    if option.value.is_empty()
                        || option.value.len() > MAX_SELECT_VALUE_BYTES
                        || invalid_text(&option.value)
                        || !values.insert(option.value.as_str())
                    {
                        return Err(PluginPlatformError::InvalidSettingsSchema);
                    }
                    validate_label(&option.label)?;
                }
                if !values.contains(default.as_str()) {
                    return Err(PluginPlatformError::InvalidSettingsSchema);
                }
                kinds.insert(key, SettingKind::Select);
            }
        }
    }
    let mut targets = BTreeSet::new();
    for visibility in &schema.target_visibility {
        if !targets.insert(visibility.target_id.as_str())
            || kinds.get(visibility.setting_key.as_str()) != Some(&SettingKind::Boolean)
        {
            return Err(PluginPlatformError::InvalidSettingsSchema);
        }
    }
    Ok(())
}

pub fn validate_settings_values(
    schema: &PluginSettingsSchema,
    values: &PluginSettingsValues,
) -> Result<()> {
    validate_settings_schema(schema)?;
    if values.len() != schema.fields.len() {
        return Err(PluginPlatformError::InvalidSettingsValues);
    }
    for field in &schema.fields {
        let Some(value) = values.get(field.key()) else {
            return Err(PluginPlatformError::InvalidSettingsValues);
        };
        if !setting_value_is_valid(field, value) {
            return Err(PluginPlatformError::InvalidSettingsValues);
        }
    }
    if values
        .keys()
        .any(|key| !schema.fields.iter().any(|field| field.key() == key))
    {
        return Err(PluginPlatformError::InvalidSettingsValues);
    }
    let serialized =
        serde_json::to_vec(values).map_err(|_| PluginPlatformError::InvalidSettingsValues)?;
    if serialized.len() > MAX_PLUGIN_SETTINGS_VALUES_BYTES {
        return Err(PluginPlatformError::InvalidSettingsValues);
    }
    Ok(())
}

pub fn settings_values_json(
    schema: &PluginSettingsSchema,
    values: &PluginSettingsValues,
) -> Result<String> {
    validate_settings_values(schema, values)?;
    serde_json::to_string(values).map_err(|_| PluginPlatformError::InvalidSettingsValues)
}

#[must_use]
pub fn default_settings_values(schema: &PluginSettingsSchema) -> PluginSettingsValues {
    schema
        .fields
        .iter()
        .map(|field| {
            let value = match field {
                PluginSettingField::Boolean { default, .. } => {
                    PluginSettingValue::Boolean(*default)
                }
                PluginSettingField::Number { default, .. } => PluginSettingValue::Number(*default),
                PluginSettingField::String { default, .. }
                | PluginSettingField::Select { default, .. } => {
                    PluginSettingValue::String(default.clone())
                }
            };
            (field.key().to_owned(), value)
        })
        .collect()
}

pub fn reconcile_settings_values(
    new_schema: &PluginSettingsSchema,
    previous_schema: Option<&PluginSettingsSchema>,
    previous_values: Option<&PluginSettingsValues>,
) -> Result<(PluginSettingsValues, Vec<String>)> {
    validate_settings_schema(new_schema)?;
    let defaults = default_settings_values(new_schema);
    let mut values = PluginSettingsValues::new();
    let mut reset_keys = Vec::new();
    for field in &new_schema.fields {
        let preserved = previous_schema
            .and_then(|schema| schema.fields.iter().find(|old| old.key() == field.key()))
            .filter(|old| setting_kind(old) == setting_kind(field))
            .and_then(|_| previous_values.and_then(|values| values.get(field.key())))
            .filter(|value| setting_value_is_valid(field, value))
            .cloned();
        if let Some(value) = preserved {
            values.insert(field.key().to_owned(), value);
        } else {
            values.insert(
                field.key().to_owned(),
                defaults
                    .get(field.key())
                    .expect("every validated field has a default")
                    .clone(),
            );
            if previous_values.is_some() {
                reset_keys.push(field.key().to_owned());
            }
        }
    }
    validate_settings_values(new_schema, &values)?;
    Ok((values, reset_keys))
}

#[must_use]
pub fn settings_target_is_visible(
    schema: &PluginSettingsSchema,
    values: &PluginSettingsValues,
    target_id: &str,
) -> bool {
    schema
        .target_visibility
        .iter()
        .find(|visibility| visibility.target_id.as_str() == target_id)
        .is_none_or(|visibility| {
            values.get(&visibility.setting_key) == Some(&PluginSettingValue::Boolean(true))
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingKind {
    Boolean,
    Number,
    String,
    Select,
}

fn setting_kind(field: &PluginSettingField) -> SettingKind {
    match field {
        PluginSettingField::Boolean { .. } => SettingKind::Boolean,
        PluginSettingField::Number { .. } => SettingKind::Number,
        PluginSettingField::String { .. } => SettingKind::String,
        PluginSettingField::Select { .. } => SettingKind::Select,
    }
}

fn setting_value_is_valid(field: &PluginSettingField, value: &PluginSettingValue) -> bool {
    match (field, value) {
        (PluginSettingField::Boolean { .. }, PluginSettingValue::Boolean(_)) => true,
        (PluginSettingField::Number { min, max, .. }, PluginSettingValue::Number(value)) => {
            value.is_finite()
                && min.is_none_or(|min| *value >= min)
                && max.is_none_or(|max| *value <= max)
        }
        (PluginSettingField::String { max_length, .. }, PluginSettingValue::String(value)) => {
            value.chars().count() <= usize::from(*max_length) && !invalid_text(value)
        }
        (PluginSettingField::Select { options, .. }, PluginSettingValue::String(value)) => {
            options.iter().any(|option| option.value == *value)
        }
        _ => false,
    }
}

fn valid_setting_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_SETTING_KEY_BYTES
        && value.is_ascii()
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn validate_label(label: &PluginSettingLabel) -> Result<()> {
    for value in [&label.en, &label.zh_cn] {
        if value.trim().is_empty() || value.len() > MAX_SETTING_LABEL_BYTES || invalid_text(value) {
            return Err(PluginPlatformError::InvalidSettingsSchema);
        }
    }
    Ok(())
}

fn invalid_text(value: &str) -> bool {
    value.chars().any(|character| {
        character.is_control()
            || matches!(
                character,
                '\u{061C}'
                    | '\u{200E}'
                    | '\u{200F}'
                    | '\u{202A}'..='\u{202E}'
                    | '\u{2066}'..='\u{2069}'
            )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> PluginSettingsSchema {
        inspect_plugin_settings(
            r#"{"schemaVersion":1,"fields":[{"type":"boolean","key":"showTerminalStatus","label":{"en":"Show status","zh-CN":"显示状态"},"default":true},{"type":"number","key":"threshold","label":{"en":"Threshold","zh-CN":"阈值"},"default":50,"min":0,"max":100},{"type":"string","key":"prefix","label":{"en":"Prefix","zh-CN":"前缀"},"default":"ok","maxLength":32},{"type":"select","key":"mode","label":{"en":"Mode","zh-CN":"模式"},"default":"compact","options":[{"value":"compact","label":{"en":"Compact","zh-CN":"紧凑"}},{"value":"full","label":{"en":"Full","zh-CN":"完整"}}]}],"targetVisibility":[{"targetId":"terminal.footer","settingKey":"showTerminalStatus"}]}"#.as_bytes(),
        )
        .unwrap()
        .schema
    }

    #[test]
    fn schema_and_values_are_strict_and_bounded() {
        let schema = schema();
        let defaults = default_settings_values(&schema);
        validate_settings_values(&schema, &defaults).unwrap();
        assert!(settings_target_is_visible(
            &schema,
            &defaults,
            "terminal.footer"
        ));
        let mut hidden = defaults.clone();
        hidden.insert(
            "showTerminalStatus".into(),
            PluginSettingValue::Boolean(false),
        );
        validate_settings_values(&schema, &hidden).unwrap();
        assert!(!settings_target_is_visible(
            &schema,
            &hidden,
            "terminal.footer"
        ));
        assert!(settings_target_is_visible(
            &schema,
            &hidden,
            "terminal.toolbar"
        ));

        let mut unknown = defaults.clone();
        unknown.insert("nested".into(), PluginSettingValue::String("{}".into()));
        assert!(validate_settings_values(&schema, &unknown).is_err());

        let invalid = r#"{"schemaVersion":1,"fields":[{"type":"string","key":"apiToken","label":{"en":"Token","zh-CN":"令牌"},"default":"","maxLength":32}]}"#.as_bytes();
        assert!(inspect_plugin_settings(invalid).is_err());
    }

    #[test]
    fn migration_preserves_only_same_kind_values_valid_under_the_new_schema() {
        let old = schema();
        let mut previous = default_settings_values(&old);
        previous.insert("threshold".into(), PluginSettingValue::Number(90.0));
        previous.insert("mode".into(), PluginSettingValue::String("full".into()));
        let mut new = old.clone();
        if let PluginSettingField::Number { max, .. } = &mut new.fields[1] {
            *max = Some(80.0);
        }
        if let PluginSettingField::Select { options, .. } = &mut new.fields[3] {
            options.retain(|option| option.value != "full");
        }
        new.fields[2] = PluginSettingField::Select {
            key: "prefix".into(),
            label: PluginSettingLabel {
                en: "Prefix".into(),
                zh_cn: "前缀".into(),
            },
            default: "compact".into(),
            options: vec![
                norishell_core_api::PluginSettingSelectOption {
                    value: "compact".into(),
                    label: PluginSettingLabel {
                        en: "Compact".into(),
                        zh_cn: "紧凑".into(),
                    },
                },
                norishell_core_api::PluginSettingSelectOption {
                    value: "ok".into(),
                    label: PluginSettingLabel {
                        en: "Okay".into(),
                        zh_cn: "正常".into(),
                    },
                },
            ],
        };
        let (migrated, reset) =
            reconcile_settings_values(&new, Some(&old), Some(&previous)).unwrap();
        assert_eq!(migrated["threshold"], PluginSettingValue::Number(50.0));
        assert_eq!(
            migrated["mode"],
            PluginSettingValue::String("compact".into())
        );
        assert_eq!(
            migrated["prefix"],
            PluginSettingValue::String("compact".into())
        );
        assert_eq!(reset, vec!["threshold", "prefix", "mode"]);
    }
}
