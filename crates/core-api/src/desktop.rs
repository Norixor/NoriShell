//! Remote desktop application contracts; protocol engines do not depend on this application-layer module.
use crate::{
    CredentialRefId, HostCreatePasswordStageId, HostId, OperationId, RequestMeta, WireSequence,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum DesktopProtocol {
    Rdp,
    Vnc,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum VncProtocolVersion {
    #[default]
    Auto,
    Rfb33,
    Rfb37,
    Rfb38,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAvailability {
    pub protocol: DesktopProtocol,
    pub available: bool,
    pub reason_key: Option<String>,
    pub presentation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopProfile {
    pub id: String,
    pub label: String,
    pub protocol: DesktopProtocol,
    pub address: String,
    pub port: u16,
    pub username: String,
    pub domain: String,
    pub host_id: Option<HostId>,
    pub gateway_host_id: Option<HostId>,
    pub credential_ref_id: Option<CredentialRefId>,
    pub width: u16,
    pub height: u16,
    pub clipboard_enabled: bool,
    #[serde(default)]
    pub audio_playback_enabled: bool,
    #[serde(default)]
    pub vnc_protocol_version: VncProtocolVersion,
    pub revision: WireSequence,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DesktopProfileSaveRequest {
    pub meta: RequestMeta,
    pub profile: DesktopProfile,
    /// A password staged through the protected Host-create flow. It is accepted only for a
    /// first save without an existing credential reference.
    #[serde(default)]
    #[ts(optional)]
    pub password_stage: Option<DesktopPasswordStage>,
}

/// Binds a desktop profile's first saved password to the exact protected staging operation.
/// This is non-secret metadata; the password itself remains in the local Vault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPasswordStage {
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub staged_password_id: HostCreatePasswordStageId,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DesktopProfileDeleteRequest {
    pub meta: RequestMeta,
    pub id: String,
    pub expected_revision: WireSequence,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DesktopOpenRequest {
    pub meta: RequestMeta,
    pub operation_id: String,
    pub profile: DesktopProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum DesktopSessionState {
    Connecting,
    NeedsInteraction,
    Running,
    Disconnecting,
    Closed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum DesktopAudioState {
    Disabled,
    Waiting,
    Ready,
    Unavailable,
    Unsupported,
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAudioMuteRequest {
    pub meta: RequestMeta,
    pub session_id: String,
    pub generation: WireSequence,
    pub muted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSessionSummary {
    pub id: String,
    pub profile: DesktopProfile,
    pub generation: WireSequence,
    pub revision: WireSequence,
    pub state: DesktopSessionState,
    pub phase: String,
    pub failure: Option<String>,
    pub width: u16,
    pub height: u16,
    pub frame_sequence: WireSequence,
    pub audio_state: DesktopAudioState,
    pub audio_muted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSessionRequest {
    pub meta: RequestMeta,
    pub session_id: String,
    pub generation: WireSequence,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DesktopInputEvent {
    Key {
        scan_code: u16,
        keysym: u32,
        down: bool,
    },
    Pointer {
        x: u16,
        y: u16,
        buttons: u8,
    },
    Wheel {
        x: u16,
        y: u16,
        delta_x: i16,
        delta_y: i16,
    },
    Text {
        text: String,
    },
    Clipboard {
        text: String,
    },
    Resize {
        width: u16,
        height: u16,
    },
    ReleaseAll,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DesktopInputRequest {
    pub meta: RequestMeta,
    pub session_id: String,
    pub generation: WireSequence,
    pub focus_epoch: WireSequence,
    pub sequence: WireSequence,
    pub input: DesktopInputEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DesktopFocusRequest {
    pub meta: RequestMeta,
    pub session_id: Option<String>,
    pub generation: Option<WireSequence>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DesktopFrameRequest {
    pub meta: RequestMeta,
    pub session_id: String,
    pub generation: WireSequence,
    pub after_sequence: WireSequence,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DesktopPromptKind {
    VaultCreate,
    VaultUnlock,
    Credentials {
        username: String,
        domain: String,
        password_only: bool,
    },
    HostKey {
        address: String,
        port: u16,
        algorithm: String,
        fingerprint: String,
    },
    Certificate {
        address: String,
        fingerprint: String,
    },
    UnencryptedVnc {
        address: String,
        gateway: bool,
    },
    KeyboardInteractive {
        name: String,
        instruction: String,
        prompts: Vec<String>,
        echo: Vec<bool>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPrompt {
    pub id: String,
    pub session_id: String,
    pub label: String,
    pub prompt: DesktopPromptKind,
}

#[derive(Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPromptDecision {
    pub id: String,
    pub approved: bool,
    pub username: Option<String>,
    pub domain: Option<String>,
    pub password: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub password_confirmation: Option<String>,
    pub answers: Vec<String>,
}

impl std::fmt::Debug for DesktopPromptDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DesktopPromptDecision")
            .field("id", &self.id)
            .field("approved", &self.approved)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{DesktopPasswordStage, DesktopProfileSaveRequest};
    use crate::{HostCreatePasswordStageId, OperationId};

    #[test]
    fn desktop_profile_request_rejects_retired_xdmcp_protocol() {
        let request = serde_json::json!({
            "meta": { "requestId": uuid::Uuid::new_v4() },
            "profile": {
                "id": uuid::Uuid::new_v4(),
                "label": "Retired XDMCP",
                "protocol": "xdmcp",
                "address": "desktop.example.test",
                "port": 177,
                "username": "user",
                "domain": "",
                "hostId": null,
                "gatewayHostId": null,
                "credentialRefId": null,
                "width": 1280,
                "height": 720,
                "clipboardEnabled": false,
                "revision": "0"
            }
        });

        assert!(serde_json::from_value::<DesktopProfileSaveRequest>(request).is_err());
    }

    #[test]
    fn desktop_profile_password_stage_is_optional_and_uses_camel_case() {
        let request = serde_json::json!({
            "meta": { "requestId": uuid::Uuid::new_v4() },
            "profile": {
                "id": uuid::Uuid::new_v4(),
                "label": "Desktop",
                "protocol": "rdp",
                "address": "desktop.example.test",
                "port": 3389,
                "username": "user",
                "domain": "",
                "hostId": null,
                "gatewayHostId": null,
                "credentialRefId": null,
                "width": 1280,
                "height": 720,
                "clipboardEnabled": false,
                "revision": "0"
            }
        });
        let without_stage: DesktopProfileSaveRequest = serde_json::from_value(request).unwrap();
        assert!(without_stage.password_stage.is_none());

        let stage = DesktopPasswordStage {
            operation_id: OperationId::new(),
            idempotency_key: "desktop-password-save".to_owned(),
            staged_password_id: HostCreatePasswordStageId::new(),
        };
        let encoded = serde_json::to_value(stage).unwrap();
        assert!(encoded.get("operationId").is_some());
        assert!(encoded.get("idempotencyKey").is_some());
        assert!(encoded.get("stagedPasswordId").is_some());
    }
}
