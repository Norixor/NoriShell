//! AndroidKeyStore-backed wrapping of the Vault auto-unlock device key.
//!
//! Kotlin `DeviceKeyStore` owns an AES-256-GCM key (alias
//! `app.norishell.vault-auto-unlock`, no user authentication). Rust only sees
//! the opaque wrapped blob (`IV || ciphertext+tag`) and the plaintext key
//! bytes; the Keystore key itself never leaves the TEE/StrongBox.

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

use crate::handle::{BridgeError, run_command};

#[derive(Debug, thiserror::Error)]
pub enum DeviceKeyError {
    /// The Keystore key is absent or permanently invalidated, or the blob no
    /// longer authenticates under the current key.
    #[error("keystore.key_missing")]
    Missing,
    /// AndroidKeyStore could not be loaded or the bridge is not ready.
    #[error("keystore.unavailable")]
    Unavailable,
    /// Any other Keystore or transport failure.
    #[error("keystore.failed")]
    Failed,
}

fn map(error: BridgeError) -> DeviceKeyError {
    match error {
        BridgeError::Rejected { code } if code == "keystore.key_missing" => DeviceKeyError::Missing,
        BridgeError::Rejected { code } if code == "keystore.unavailable" => {
            DeviceKeyError::Unavailable
        }
        BridgeError::NotReady | BridgeError::UiThread | BridgeError::ActivityUnavailable => {
            DeviceKeyError::Unavailable
        }
        _ => DeviceKeyError::Failed,
    }
}

#[derive(Serialize)]
struct DataRequest<'a> {
    data: &'a str,
}

#[derive(Deserialize)]
struct DataResponse {
    data: String,
}

fn exchange(command: &str, input: &[u8]) -> Result<Vec<u8>, DeviceKeyError> {
    let encoded = STANDARD.encode(input);
    let response: DataResponse =
        run_command(command, DataRequest { data: &encoded }).map_err(map)?;
    STANDARD
        .decode(response.data)
        .map_err(|_| DeviceKeyError::Failed)
}

/// Encrypts `plaintext` under the Keystore key, creating the key if absent.
pub fn device_key_wrap(plaintext: &[u8]) -> Result<Vec<u8>, DeviceKeyError> {
    exchange("deviceKeyWrap", plaintext)
}

/// Decrypts a blob produced by [`device_key_wrap`]. Never creates a key.
pub fn device_key_unwrap(blob: &[u8]) -> Result<Vec<u8>, DeviceKeyError> {
    exchange("deviceKeyUnwrap", blob)
}

/// Deletes the Keystore key. Deleting an absent key succeeds.
pub fn device_key_delete() -> Result<(), DeviceKeyError> {
    run_command::<serde_json::Value>("deviceKeyDelete", serde_json::json!({}))
        .map(|_| ())
        .map_err(map)
}
