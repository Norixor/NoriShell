//! Typed, bounded protocol-provider declarations and isolated-host wire frames.
//!
//! A declaration is package metadata only. It never grants a plugin access to a
//! socket, serial device, or any other resource; Core still mediates every API
//! call and resource handle.

use std::fmt;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    PluginApiCall, PluginApiErrorCode, PluginApiReply, PluginApiResourceEvent, PluginSettingLabel,
    PluginSettingsSchema, PluginSettingsValues,
};

pub const PLUGIN_PROTOCOL_CATALOG_SCHEMA_VERSION: u16 = 1;
pub const MAX_PLUGIN_PROTOCOL_FRAME_BYTES: usize = 64 * 1024;
pub const MAX_PLUGIN_PROTOCOL_PROVIDERS: usize = 8;
pub const MAX_PLUGIN_PROTOCOL_OUTPUTS: usize = 64;
pub const MAX_PLUGIN_PROTOCOL_RESOURCE_EVENTS: usize = 64;
pub const MAX_PLUGIN_PROTOCOL_ID_BYTES: usize = 64;
pub const MAX_PLUGIN_PROTOCOL_HANDLE_BYTES: usize = 128;
pub const MAX_PLUGIN_PROTOCOL_EVENT_ID_BYTES: usize = 128;
pub const MAX_PLUGIN_PROTOCOL_CALL_ID_BYTES: usize = 128;
pub const MAX_PLUGIN_PROTOCOL_ROWS: u16 = 1_000;
pub const MAX_PLUGIN_PROTOCOL_COLUMNS: u16 = 1_000;

/// Package-fixed catalog read from `assets/protocols.json` during package
/// inspection. It is intentionally not a runtime registration mechanism.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginProtocolCatalog {
    pub schema_version: u16,
    pub providers: Vec<PluginProtocolProvider>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginProtocolProvider {
    pub id: String,
    pub label: PluginSettingLabel,
    /// Non-secret primitive configuration only. Platform validation reuses the
    /// existing package settings validator for this schema.
    pub configuration: PluginSettingsSchema,
    pub features: PluginProtocolFeatures,
    pub resources: Vec<PluginProtocolResource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginProtocolFeatures {
    pub terminal: bool,
    pub resize: PluginProtocolResizeSupport,
    pub reconnect: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginProtocolResizeSupport {
    Supported,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginProtocolResource {
    Tcp,
    Tls,
    Websocket,
    Serial,
}

/// Core-to-plugin event for one protocol connection. The handles and IDs are
/// opaque correlation values, never grants or identities supplied by a guest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginProtocolEvent {
    pub connection_handle: String,
    pub event_id: String,
    pub event: PluginProtocolEventKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginProtocolEventKind {
    Connect {
        provider_id: String,
        configuration: PluginSettingsValues,
        rows: u16,
        cols: u16,
    },
    Input {
        data_base64: String,
    },
    Resize {
        rows: u16,
        cols: u16,
    },
    Resource {
        resource_handle: String,
        events: Vec<PluginApiResourceEvent>,
    },
    ApiResult {
        call_id: String,
        reply: PluginApiReply,
    },
    Close {
        reason: PluginProtocolCloseReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginProtocolCloseReason {
    User,
    Revoked,
    Shutdown,
}

/// Plugin-to-Core response for one event. A response may ask Core to broker at
/// most one existing API call; it cannot directly open a declared resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginProtocolResponse {
    pub connection_handle: String,
    pub event_id: String,
    pub outputs: Vec<PluginProtocolOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub call: Option<PluginApiCall>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginProtocolOutput {
    Output {
        data_base64: String,
    },
    Ready {},
    Exit {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        code: Option<i64>,
    },
    Fail {
        stable_code: PluginApiErrorCode,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginProtocolValidationError;

impl fmt::Display for PluginProtocolValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("plugin protocol wire is invalid")
    }
}

impl std::error::Error for PluginProtocolValidationError {}

/// Parses one untrusted Core-to-plugin frame and checks its fixed wire limits.
pub fn parse_plugin_protocol_event(
    bytes: &[u8],
) -> Result<PluginProtocolEvent, PluginProtocolValidationError> {
    if bytes.is_empty() || bytes.len() > MAX_PLUGIN_PROTOCOL_FRAME_BYTES {
        return Err(PluginProtocolValidationError);
    }
    let event = serde_json::from_slice(bytes).map_err(|_| PluginProtocolValidationError)?;
    validate_plugin_protocol_event(&event)?;
    Ok(event)
}

/// Parses one untrusted plugin-to-Core frame and checks its fixed wire limits.
pub fn parse_plugin_protocol_response(
    bytes: &[u8],
) -> Result<PluginProtocolResponse, PluginProtocolValidationError> {
    if bytes.is_empty() || bytes.len() > MAX_PLUGIN_PROTOCOL_FRAME_BYTES {
        return Err(PluginProtocolValidationError);
    }
    let response = serde_json::from_slice(bytes).map_err(|_| PluginProtocolValidationError)?;
    validate_plugin_protocol_response(&response)?;
    Ok(response)
}

pub fn validate_plugin_protocol_event(
    event: &PluginProtocolEvent,
) -> Result<(), PluginProtocolValidationError> {
    valid_handle(&event.connection_handle)?;
    valid_text_id(&event.event_id, MAX_PLUGIN_PROTOCOL_EVENT_ID_BYTES)?;
    match &event.event {
        PluginProtocolEventKind::Connect {
            provider_id,
            rows,
            cols,
            ..
        } => {
            valid_text_id(provider_id, MAX_PLUGIN_PROTOCOL_ID_BYTES)?;
            valid_dimensions(*rows, *cols)?;
        }
        PluginProtocolEventKind::Input { data_base64 } => valid_base64(data_base64)?,
        PluginProtocolEventKind::Resize { rows, cols } => valid_dimensions(*rows, *cols)?,
        PluginProtocolEventKind::Resource {
            resource_handle,
            events,
        } => {
            valid_handle(resource_handle)?;
            if events.len() > MAX_PLUGIN_PROTOCOL_RESOURCE_EVENTS {
                return Err(PluginProtocolValidationError);
            }
        }
        PluginProtocolEventKind::ApiResult { call_id, reply } => {
            valid_text_id(call_id, MAX_PLUGIN_PROTOCOL_CALL_ID_BYTES)?;
            if reply.call_id != *call_id {
                return Err(PluginProtocolValidationError);
            }
        }
        PluginProtocolEventKind::Close { .. } => {}
    }
    serialized_fits(event)
}

pub fn validate_plugin_protocol_response(
    response: &PluginProtocolResponse,
) -> Result<(), PluginProtocolValidationError> {
    valid_handle(&response.connection_handle)?;
    valid_text_id(&response.event_id, MAX_PLUGIN_PROTOCOL_EVENT_ID_BYTES)?;
    if response.outputs.len() > MAX_PLUGIN_PROTOCOL_OUTPUTS {
        return Err(PluginProtocolValidationError);
    }
    for output in &response.outputs {
        if let PluginProtocolOutput::Output { data_base64 } = output {
            valid_base64(data_base64)?;
        }
    }
    if let Some(call) = &response.call {
        valid_text_id(&call.call_id, MAX_PLUGIN_PROTOCOL_CALL_ID_BYTES)?;
    }
    serialized_fits(response)
}

fn valid_dimensions(rows: u16, cols: u16) -> Result<(), PluginProtocolValidationError> {
    if !(1..=MAX_PLUGIN_PROTOCOL_ROWS).contains(&rows)
        || !(1..=MAX_PLUGIN_PROTOCOL_COLUMNS).contains(&cols)
    {
        return Err(PluginProtocolValidationError);
    }
    Ok(())
}

fn valid_handle(value: &str) -> Result<(), PluginProtocolValidationError> {
    valid_text_id(value, MAX_PLUGIN_PROTOCOL_HANDLE_BYTES)
}

fn valid_text_id(value: &str, maximum: usize) -> Result<(), PluginProtocolValidationError> {
    if value.is_empty()
        || value.len() > maximum
        || !value.is_ascii()
        || value.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(PluginProtocolValidationError);
    }
    Ok(())
}

fn valid_base64(value: &str) -> Result<(), PluginProtocolValidationError> {
    if value.len() > MAX_PLUGIN_PROTOCOL_FRAME_BYTES
        || !value.len().is_multiple_of(4)
        || value.bytes().enumerate().any(|(index, byte)| {
            !(byte.is_ascii_alphanumeric()
                || matches!(byte, b'+' | b'/')
                || (byte == b'=' && index + 2 >= value.len()))
        })
        || value
            .as_bytes()
            .iter()
            .position(|byte| *byte == b'=')
            .is_some_and(|index| value.as_bytes()[index..].iter().any(|byte| *byte != b'='))
    {
        return Err(PluginProtocolValidationError);
    }
    Ok(())
}

fn serialized_fits<T: Serialize>(value: &T) -> Result<(), PluginProtocolValidationError> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len() <= MAX_PLUGIN_PROTOCOL_FRAME_BYTES)
        .map_err(|_| PluginProtocolValidationError)
        .and_then(|fits| fits.then_some(()).ok_or(PluginProtocolValidationError))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_protocol_frames_are_strict_and_bounded() {
        let event = r#"{"connectionHandle":"connection","eventId":"event","event":{"kind":"input","dataBase64":"aGk="}}"#;
        assert!(parse_plugin_protocol_event(event.as_bytes()).is_ok());
        assert!(parse_plugin_protocol_event(
            br#"{"connectionHandle":"connection","eventId":"event","unexpected":true,"event":{"kind":"input","dataBase64":"aGk="}}"#,
        )
        .is_err());
        assert!(parse_plugin_protocol_event(
            br#"{"connectionHandle":"connection","eventId":"event","event":{"kind":"input","dataBase64":"not-base64"}}"#,
        )
        .is_err());
        assert!(
            parse_plugin_protocol_event(&vec![b'x'; MAX_PLUGIN_PROTOCOL_FRAME_BYTES + 1]).is_err()
        );
    }

    #[test]
    fn response_rejects_oversized_outputs_and_unknown_fields() {
        let output = PluginProtocolOutput::Output {
            data_base64: "a".repeat(MAX_PLUGIN_PROTOCOL_FRAME_BYTES),
        };
        assert!(
            validate_plugin_protocol_response(&PluginProtocolResponse {
                connection_handle: "connection".into(),
                event_id: "event".into(),
                outputs: vec![output],
                call: None,
                complete: false,
            })
            .is_err()
        );
        assert!(parse_plugin_protocol_response(
            br#"{"connectionHandle":"connection","eventId":"event","outputs":[],"complete":true,"extra":false}"#,
        )
        .is_err());
    }

    #[test]
    fn api_result_correlation_cannot_be_rebound() {
        let event = PluginProtocolEvent {
            connection_handle: "connection".into(),
            event_id: "event".into(),
            event: PluginProtocolEventKind::ApiResult {
                call_id: "expected".into(),
                reply: PluginApiReply {
                    call_id: "different".into(),
                    outcome: crate::PluginApiOutcome::Failed {
                        code: PluginApiErrorCode::InvalidRequest,
                    },
                },
            },
        };
        assert!(validate_plugin_protocol_event(&event).is_err());
    }
}
