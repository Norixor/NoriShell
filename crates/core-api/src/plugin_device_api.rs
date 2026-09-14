//! Core-owned serial-device declarations, plans and resource events.
//!
//! Plugin JSON can name only an opaque candidate or an already-open resource.
//! Core resolves a native-device confirmation into FrozenSerialPlan, which
//! deliberately is not serializable and is revalidated before opening a port.

use std::{
    fmt,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::PluginApiErrorCode;

pub const MAX_PLUGIN_SERIAL_CANDIDATE_ID_BYTES: usize = 80;
pub const MAX_PLUGIN_SERIAL_LABEL_BYTES: usize = 160;
pub const MAX_PLUGIN_SERIAL_METADATA_BYTES: usize = 160;
pub const MAX_PLUGIN_SERIAL_SEND_BYTES: usize = 8 * 1024;
pub const MIN_PLUGIN_SERIAL_BAUD_RATE: u32 = 300;
pub const MAX_PLUGIN_SERIAL_BAUD_RATE: u32 = 4_000_000;

/// Host-rendered, non-authoritative device information. candidate_id is an
/// opaque, short-lived Core lookup key, never a serial path or an open grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSerialDeviceCandidate {
    pub candidate_id: String,
    pub metadata: PluginSerialPortMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSerialPortMetadata {
    pub label: String,
    pub kind: PluginSerialPortKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub manufacturer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub product: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub usb_vendor_id: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub usb_product_id: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginSerialPortKind {
    Usb,
    Pci,
    Bluetooth,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginSerialDataBits {
    Five,
    Six,
    Seven,
    Eight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginSerialParity {
    None,
    Odd,
    Even,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginSerialStopBits {
    One,
    Two,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginSerialFlowControl {
    None,
    Software,
    Hardware,
}

/// Bounded line settings selected in the native Core surface before a device
/// plan is frozen. These settings carry no path, identity or authorization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSerialSettings {
    pub baud_rate: u32,
    pub data_bits: PluginSerialDataBits,
    pub parity: PluginSerialParity,
    pub stop_bits: PluginSerialStopBits,
    pub flow_control: PluginSerialFlowControl,
}

impl PluginSerialSettings {
    pub fn validate(self) -> Result<(), PluginApiErrorCode> {
        if !(MIN_PLUGIN_SERIAL_BAUD_RATE..=MAX_PLUGIN_SERIAL_BAUD_RATE).contains(&self.baud_rate) {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        Ok(())
    }
}

/// This is constructed only by Core after native device confirmation. It is
/// intentionally not a wire DTO: guest JSON can neither supply a device path
/// nor retain a plan after its approval and device identity become stale.
#[derive(Clone, PartialEq, Eq)]
pub struct FrozenSerialPlan {
    canonical_path: PathBuf,
    platform_identity: String,
    settings: PluginSerialSettings,
}

impl fmt::Debug for FrozenSerialPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FrozenSerialPlan")
            .field("baud_rate", &self.settings.baud_rate)
            .finish_non_exhaustive()
    }
}

impl FrozenSerialPlan {
    /// Core calls this only after it resolved an opaque candidate and completed
    /// native confirmation. The driver repeats the identity check immediately
    /// before opening the device.
    pub fn new(
        canonical_path: PathBuf,
        platform_identity: String,
        settings: PluginSerialSettings,
    ) -> Result<Self, PluginApiErrorCode> {
        let plan = Self {
            canonical_path,
            platform_identity,
            settings,
        };
        plan.validate_bounds()?;
        Ok(plan)
    }

    pub fn validate_bounds(&self) -> Result<(), PluginApiErrorCode> {
        if !self.canonical_path.is_absolute()
            || self.platform_identity.is_empty()
            || self.platform_identity.len() > 128
            || !self
                .platform_identity
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        self.settings.validate()
    }

    #[must_use]
    pub fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }

    #[must_use]
    pub fn platform_identity(&self) -> &str {
        &self.platform_identity
    }

    #[must_use]
    pub const fn settings(&self) -> PluginSerialSettings {
        self.settings
    }
}

/// Bounded guest bytes sent to an already owner-scoped serial resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSerialSendRequest {
    pub handle: String,
    pub data_base64: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginSerialEvent {
    Opened {},
    Data { data_base64: String },
    Closed {},
    Error { stable_code: PluginApiErrorCode },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> PluginSerialSettings {
        PluginSerialSettings {
            baud_rate: 115_200,
            data_bits: PluginSerialDataBits::Eight,
            parity: PluginSerialParity::None,
            stop_bits: PluginSerialStopBits::One,
            flow_control: PluginSerialFlowControl::None,
        }
    }

    #[test]
    fn serial_plan_is_non_wire_and_requires_a_canonical_identity() {
        assert!(FrozenSerialPlan::new("relative".into(), "a".repeat(64), settings(),).is_err());
        assert!(
            FrozenSerialPlan::new("/dev/tty.example".into(), "identity".into(), settings(),)
                .is_err()
        );
        assert!(
            FrozenSerialPlan::new("/dev/tty.example".into(), "a".repeat(64), settings(),).is_ok()
        );
    }

    #[test]
    fn serial_wire_rejects_paths_and_unbounded_settings() {
        assert!(serde_json::from_str::<PluginSerialDeviceCandidate>(
            r#"{"candidateId":"candidate","metadata":{"label":"USB serial","kind":"usb"},"path":"/dev/ttyUSB0"}"#,
        )
        .is_err());
        assert!(
            PluginSerialSettings {
                baud_rate: MAX_PLUGIN_SERIAL_BAUD_RATE + 1,
                ..settings()
            }
            .validate()
            .is_err()
        );
    }
}
