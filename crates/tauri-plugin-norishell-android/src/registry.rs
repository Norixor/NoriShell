//! Kotlin -> Rust callback registry.
//!
//! The application registers each callback once during setup. A JNI call that
//! arrives before (or without) registration fails closed with a stable code;
//! callbacks run on the calling Kotlin thread and must not block on
//! [`crate::run_command`] when that thread is the UI thread.

use std::sync::OnceLock;

/// A served document or asset for a Kotlin-hosted WebView.
#[derive(Clone, Debug)]
pub struct AssetResponse {
    pub status: u16,
    pub mime_type: String,
    /// Extra response headers (CSP etc.). Kotlin applies them verbatim.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// `(surface label, command, payload JSON) -> response JSON`.
pub type SecureDispatcher = dyn Fn(&str, &str, &str) -> String + Send + Sync;
/// `(request path) -> asset`, for `https://secure.norishell.localhost/<path>`.
pub type AssetReader = dyn Fn(&str) -> Option<AssetResponse> + Send + Sync;
/// `(surface label, request path) -> plugin-isolated document`.
pub type IsolatedDocumentReader = dyn Fn(&str, &str) -> Option<AssetResponse> + Send + Sync;
/// `(surface label)`: the Kotlin surface was destroyed (user, system or finish).
pub type SurfaceDestroyed = dyn Fn(&str) + Send + Sync;
/// `(action)`: notification / service action, see the bridge contract.
pub type NotificationAction = dyn Fn(&str) + Send + Sync;
/// `(event id)`: the user tapped a resource notification posted through
/// `notificationsPost`. The id is the host-owned event id, validated by Kotlin.
pub type NotificationClick = dyn Fn(&str) + Send + Sync;

pub(crate) static SECURE_DISPATCHER: OnceLock<Box<SecureDispatcher>> = OnceLock::new();
pub(crate) static ASSET_READER: OnceLock<Box<AssetReader>> = OnceLock::new();
pub(crate) static ISOLATED_DOCUMENT: OnceLock<Box<IsolatedDocumentReader>> = OnceLock::new();
pub(crate) static SURFACE_DESTROYED: OnceLock<Box<SurfaceDestroyed>> = OnceLock::new();
pub(crate) static NOTIFICATION_ACTION: OnceLock<Box<NotificationAction>> = OnceLock::new();
pub(crate) static NOTIFICATION_CLICK: OnceLock<Box<NotificationClick>> = OnceLock::new();

/// Returned when a callback slot is already taken; the first registration wins.
#[derive(Debug, thiserror::Error)]
#[error("android_bridge.callback_already_registered")]
pub struct AlreadyRegistered;

fn set<T: ?Sized>(slot: &OnceLock<Box<T>>, value: Box<T>) -> Result<(), AlreadyRegistered> {
    slot.set(value).map_err(|_| AlreadyRegistered)
}

pub fn register_secure_dispatcher(
    callback: impl Fn(&str, &str, &str) -> String + Send + Sync + 'static,
) -> Result<(), AlreadyRegistered> {
    set(&SECURE_DISPATCHER, Box::new(callback))
}

pub fn register_asset_reader(
    callback: impl Fn(&str) -> Option<AssetResponse> + Send + Sync + 'static,
) -> Result<(), AlreadyRegistered> {
    set(&ASSET_READER, Box::new(callback))
}

pub fn register_isolated_document(
    callback: impl Fn(&str, &str) -> Option<AssetResponse> + Send + Sync + 'static,
) -> Result<(), AlreadyRegistered> {
    set(&ISOLATED_DOCUMENT, Box::new(callback))
}

pub fn register_surface_destroyed(
    callback: impl Fn(&str) + Send + Sync + 'static,
) -> Result<(), AlreadyRegistered> {
    set(&SURFACE_DESTROYED, Box::new(callback))
}

pub fn register_notification_action(
    callback: impl Fn(&str) + Send + Sync + 'static,
) -> Result<(), AlreadyRegistered> {
    set(&NOTIFICATION_ACTION, Box::new(callback))
}

pub fn register_notification_click(
    callback: impl Fn(&str) + Send + Sync + 'static,
) -> Result<(), AlreadyRegistered> {
    set(&NOTIFICATION_CLICK, Box::new(callback))
}

/// Frames an asset for JNI: `u32 BE header length | header JSON | body`.
/// Kotlin `NoriShellNative.decodeAsset` is the only decoder.
pub(crate) fn encode_asset(asset: &AssetResponse) -> Vec<u8> {
    let headers: serde_json::Map<String, serde_json::Value> = asset
        .headers
        .iter()
        .map(|(name, value)| (name.clone(), serde_json::Value::String(value.clone())))
        .collect();
    let header = serde_json::json!({
        "status": asset.status,
        "mimeType": asset.mime_type,
        "headers": headers,
    })
    .to_string();
    let header = header.as_bytes();
    let mut framed = Vec::with_capacity(4 + header.len() + asset.body.len());
    framed.extend_from_slice(&(header.len() as u32).to_be_bytes());
    framed.extend_from_slice(header);
    framed.extend_from_slice(&asset.body);
    framed
}
