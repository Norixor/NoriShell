//! Validation for the fixed `assets/protocols.json` package asset.

use std::collections::BTreeSet;

use norishell_core_api::{
    MAX_PLUGIN_PROTOCOL_ID_BYTES, MAX_PLUGIN_PROTOCOL_PROVIDERS,
    PLUGIN_PROTOCOL_CATALOG_SCHEMA_VERSION, PluginProtocolCatalog, PluginProtocolProvider,
};
use sha2::{Digest, Sha256};

use crate::{PluginPlatformError, Result, package::lower_hex, validate_settings_schema};

pub const PLUGIN_PROTOCOLS_ASSET_PATH: &str = "assets/protocols.json";
pub const MAX_PLUGIN_PROTOCOL_CATALOG_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct InspectedPluginProtocols {
    pub catalog: PluginProtocolCatalog,
    pub catalog_json: String,
    pub catalog_sha256: String,
}

/// Parses only the bytes held in an already inspected package snapshot. The
/// catalog records eligible provider shapes; it never grants resources.
pub fn inspect_plugin_protocols(bytes: &[u8]) -> Result<InspectedPluginProtocols> {
    if bytes.is_empty() || bytes.len() > MAX_PLUGIN_PROTOCOL_CATALOG_BYTES {
        return Err(PluginPlatformError::InvalidProtocolCatalog);
    }
    let catalog: PluginProtocolCatalog =
        serde_json::from_slice(bytes).map_err(|_| PluginPlatformError::InvalidProtocolCatalog)?;
    validate_plugin_protocol_catalog(&catalog)?;
    let catalog_json =
        serde_json::to_string(&catalog).map_err(|_| PluginPlatformError::InvalidProtocolCatalog)?;
    if catalog_json.len() > MAX_PLUGIN_PROTOCOL_CATALOG_BYTES {
        return Err(PluginPlatformError::InvalidProtocolCatalog);
    }
    Ok(InspectedPluginProtocols {
        catalog,
        catalog_sha256: lower_hex(&Sha256::digest(catalog_json.as_bytes())),
        catalog_json,
    })
}

pub fn validate_plugin_protocol_catalog(catalog: &PluginProtocolCatalog) -> Result<()> {
    if catalog.schema_version != PLUGIN_PROTOCOL_CATALOG_SCHEMA_VERSION
        || catalog.providers.len() > MAX_PLUGIN_PROTOCOL_PROVIDERS
    {
        return Err(PluginPlatformError::InvalidProtocolCatalog);
    }
    let mut ids = BTreeSet::new();
    for provider in &catalog.providers {
        validate_provider(provider)?;
        if !ids.insert(provider.id.as_str()) {
            return Err(PluginPlatformError::InvalidProtocolCatalog);
        }
    }
    Ok(())
}

fn validate_provider(provider: &PluginProtocolProvider) -> Result<()> {
    if !valid_provider_id(&provider.id)
        || !provider.features.terminal
        || provider.resources.len() > 4
        || provider.resources.iter().collect::<BTreeSet<_>>().len() != provider.resources.len()
    {
        return Err(PluginPlatformError::InvalidProtocolCatalog);
    }
    validate_settings_schema(&provider.configuration)
        .map_err(|_| PluginPlatformError::InvalidProtocolCatalog)?;
    validate_label(&provider.label)
}

fn valid_provider_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_PLUGIN_PROTOCOL_ID_BYTES
        && value.is_ascii()
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn validate_label(label: &norishell_core_api::PluginSettingLabel) -> Result<()> {
    for value in [&label.en, &label.zh_cn] {
        if value.trim().is_empty()
            || value.len() > 512
            || value.chars().any(|character| {
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
        {
            return Err(PluginPlatformError::InvalidProtocolCatalog);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"{
        "schemaVersion": 1,
        "providers": [{
            "id": "telnet",
            "label": {"en": "Telnet", "zh-CN": "Telnet"},
            "configuration": {
                "schemaVersion": 1,
                "fields": [{
                    "type": "number", "key": "port",
                    "label": {"en": "Port", "zh-CN": "端口"},
                    "default": 23, "min": 1, "max": 65535
                }]
            },
            "features": {"terminal": true, "resize": "supported", "reconnect": true},
            "resources": ["tcp"]
        }]
    }"#;

    #[test]
    fn catalog_is_strict_bounded_and_reuses_settings_validation() {
        let inspected = inspect_plugin_protocols(VALID.as_bytes()).expect("valid catalog");
        assert_eq!(inspected.catalog.providers[0].id, "telnet");
        assert!(
            inspect_plugin_protocols(
                br#"{"schemaVersion":1,"providers":[],"resourceApproval":true}"#,
            )
            .is_err()
        );
        assert!(inspect_plugin_protocols(
            r#"{"schemaVersion":1,"providers":[{"id":"telnet","label":{"en":"Telnet","zh-CN":"Telnet"},"configuration":{"schemaVersion":1,"fields":[{"type":"string","key":"apiToken","label":{"en":"Token","zh-CN":"令牌"},"default":"","maxLength":1}]},"features":{"terminal":true,"resize":"supported","reconnect":true},"resources":["tcp"]}]}"#.as_bytes(),
        )
        .is_err());
    }

    #[test]
    fn duplicate_provider_ids_and_resources_are_rejected() {
        let duplicate_id = VALID.replace(
            "]\n    }",
            ", {\"id\":\"telnet\",\"label\":{\"en\":\"Telnet\",\"zh-CN\":\"Telnet\"},\"configuration\":{\"schemaVersion\":1,\"fields\":[{\"type\":\"boolean\",\"key\":\"enabled\",\"label\":{\"en\":\"Enabled\",\"zh-CN\":\"启用\"},\"default\":true}]},\"features\":{\"terminal\":true,\"resize\":\"unsupported\",\"reconnect\":false},\"resources\":[\"serial\"]}]\n    }",
        );
        assert!(inspect_plugin_protocols(duplicate_id.as_bytes()).is_err());
        assert!(
            inspect_plugin_protocols(VALID.replace("[\"tcp\"]", "[\"tcp\", \"tcp\"]").as_bytes())
                .is_err()
        );
    }

    #[test]
    fn oversized_catalog_and_non_terminal_provider_are_rejected() {
        assert!(
            inspect_plugin_protocols(&vec![b' '; MAX_PLUGIN_PROTOCOL_CATALOG_BYTES + 1]).is_err()
        );
        assert!(
            inspect_plugin_protocols(
                VALID
                    .replace("\"terminal\": true", "\"terminal\": false")
                    .as_bytes()
            )
            .is_err()
        );
    }
}
