use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{RequestMeta, WireSequence};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum VaultState {
    Missing,
    Locked,
    Unlocked,
    RequiresReload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum VaultUnlockPolicy {
    CurrentSession,
    Automatic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum VaultAutoUnlockFailure {
    SecureStorageUnavailable,
    DeviceKeyMissing,
    DeviceUnlockRejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatus {
    pub state: VaultState,
    pub vault_id: Option<String>,
    pub revision: Option<WireSequence>,
    pub entry_count: Option<u32>,
    pub unlock_policy: VaultUnlockPolicy,
    pub auto_unlock_failure: Option<VaultAutoUnlockFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatusRequest {
    pub meta: RequestMeta,
}

#[derive(Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct VaultCreateRequest {
    pub meta: RequestMeta,
    pub password: String,
    pub password_confirmation: String,
}

impl std::fmt::Debug for VaultCreateRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VaultCreateRequest")
            .field("meta", &self.meta)
            .field("password", &"<redacted>")
            .field("password_confirmation", &"<redacted>")
            .finish()
    }
}

#[derive(Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct VaultUnlockRequest {
    pub meta: RequestMeta,
    pub password: String,
}

impl std::fmt::Debug for VaultUnlockRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VaultUnlockRequest")
            .field("meta", &self.meta)
            .field("password", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct VaultLockRequest {
    pub meta: RequestMeta,
}

#[derive(Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct VaultAutoUnlockEnableRequest {
    pub meta: RequestMeta,
    pub password: String,
}

impl std::fmt::Debug for VaultAutoUnlockEnableRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VaultAutoUnlockEnableRequest")
            .field("meta", &self.meta)
            .field("password", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct VaultAutoUnlockDisableRequest {
    pub meta: RequestMeta,
}

#[derive(Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct VaultChangePasswordRequest {
    pub meta: RequestMeta,
    pub current_password: String,
    pub new_password: String,
    pub new_password_confirmation: String,
}

impl std::fmt::Debug for VaultChangePasswordRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VaultChangePasswordRequest")
            .field("meta", &self.meta)
            .field("current_password", &"<redacted>")
            .field("new_password", &"<redacted>")
            .field("new_password_confirmation", &"<redacted>")
            .finish()
    }
}
