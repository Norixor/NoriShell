//! Core-owned, package-declared plugin settings.
//!
//! The schema is immutable package data. Only the host can read or replace
//! values; plugin guests receive a bounded values projection in protocol 12.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{PluginExtensionTargetId, PluginId, RequestMeta, WireSequence};

pub const PLUGIN_SETTINGS_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSettingLabel {
    pub en: String,
    #[serde(rename = "zh-CN")]
    #[ts(rename = "zh-CN")]
    pub zh_cn: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(untagged)]
pub enum PluginSettingValue {
    Boolean(bool),
    Number(f64),
    String(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSettingSelectOption {
    pub value: String,
    pub label: PluginSettingLabel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginSettingField {
    Boolean {
        key: String,
        label: PluginSettingLabel,
        default: bool,
    },
    Number {
        key: String,
        label: PluginSettingLabel,
        default: f64,
        min: Option<f64>,
        max: Option<f64>,
    },
    String {
        key: String,
        label: PluginSettingLabel,
        default: String,
        max_length: u16,
    },
    Select {
        key: String,
        label: PluginSettingLabel,
        default: String,
        options: Vec<PluginSettingSelectOption>,
    },
}

impl PluginSettingField {
    #[must_use]
    pub fn key(&self) -> &str {
        match self {
            Self::Boolean { key, .. }
            | Self::Number { key, .. }
            | Self::String { key, .. }
            | Self::Select { key, .. } => key,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSettingsTargetVisibility {
    pub target_id: PluginExtensionTargetId,
    pub setting_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSettingsSchema {
    pub schema_version: u16,
    pub fields: Vec<PluginSettingField>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_visibility: Vec<PluginSettingsTargetVisibility>,
}

pub type PluginSettingsValues = BTreeMap<String, PluginSettingValue>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSettingsSnapshot {
    pub plugin_id: PluginId,
    pub package_sha256: String,
    pub installed_state_version: WireSequence,
    pub schema_sha256: String,
    pub revision: WireSequence,
    pub schema: PluginSettingsSchema,
    pub values: PluginSettingsValues,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSettingsGetRequest {
    pub meta: RequestMeta,
    pub plugin_id: PluginId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSettingsReplaceRequest {
    pub meta: RequestMeta,
    pub plugin_id: PluginId,
    pub expected_package_sha256: String,
    pub expected_installed_state_version: WireSequence,
    pub expected_schema_sha256: String,
    pub expected_revision: WireSequence,
    pub values: PluginSettingsValues,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSettingsResetRequest {
    pub meta: RequestMeta,
    pub plugin_id: PluginId,
    pub expected_package_sha256: String,
    pub expected_installed_state_version: WireSequence,
    pub expected_schema_sha256: String,
    pub expected_revision: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSettingsChanged {
    pub plugin_id: PluginId,
    pub revision: WireSequence,
}

/// Non-secret setting keys shared by package validation and workspace persistence.
pub fn sensitive_plugin_setting_key(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase().replace(['.', '_', '-'], "");
    [
        "password",
        "passphrase",
        "secret",
        "token",
        "apikey",
        "privatekey",
        "script",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}
