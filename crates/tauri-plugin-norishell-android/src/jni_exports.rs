//! JNI entry points for `app.norishell.android.bridge.NoriShellNative`.
//!
//! Every function upgrades the raw env with `with_env` (which also catches
//! panics) and never lets a Rust error cross into Java as anything other than
//! a null/false/default return or a stable JSON error code.

use std::sync::atomic::Ordering;

use jni::{
    EnvUnowned,
    errors::LogErrorAndDefault,
    objects::{JObject, JString},
};

use crate::{
    handle::{UI_THREAD, VERIFIER_READY},
    notifications::ACTIVITY_RESUMED,
    registry::{
        ASSET_READER, ISOLATED_DOCUMENT, NOTIFICATION_ACTION, NOTIFICATION_CLICK,
        SECURE_DISPATCHER, SURFACE_DESTROYED, encode_asset,
    },
};

const UNREGISTERED: &str = r#"{"ok":false,"code":"android_bridge.dispatcher_unavailable"}"#;

/// `fun nativeInit(context: Context): Boolean` — called once from the plugin
/// constructor on the UI thread. Initializes rustls-platform-verifier with the
/// application context and records the UI thread.
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_norishell_android_bridge_NoriShellNative_nativeInit<'caller>(
    mut unowned: EnvUnowned<'caller>,
    _this: JObject<'caller>,
    context: JObject<'caller>,
) -> bool {
    let _ = UI_THREAD.set(std::thread::current().id());
    unowned
        .with_env(|env| -> Result<bool, jni::errors::Error> {
            if VERIFIER_READY.load(Ordering::Acquire) {
                return Ok(true);
            }
            match rustls_platform_verifier::android::init_with_env(env, context) {
                Ok(()) => {
                    VERIFIER_READY.store(true, Ordering::Release);
                    crate::log::info("platform verifier initialized");
                    Ok(true)
                }
                Err(_) => {
                    if env.exception_check() {
                        env.exception_clear();
                    }
                    crate::log::warn("platform verifier initialization failed");
                    Ok(false)
                }
            }
        })
        .resolve::<LogErrorAndDefault>()
}

/// `fun nativeSecureDispatch(label: String, command: String, payload: String): String`
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_norishell_android_bridge_NoriShellNative_nativeSecureDispatch<
    'caller,
>(
    mut unowned: EnvUnowned<'caller>,
    _this: JObject<'caller>,
    label: JString<'caller>,
    command: JString<'caller>,
    payload: JString<'caller>,
) -> JObject<'caller> {
    unowned
        .with_env(|env| -> Result<JObject<'caller>, jni::errors::Error> {
            let label = label.try_to_string(env)?;
            let command = command.try_to_string(env)?;
            let mut payload = payload.try_to_string(env)?;
            let response = match SECURE_DISPATCHER.get() {
                Some(dispatch) => dispatch(&label, &command, &payload),
                None => UNREGISTERED.to_owned(),
            };
            // The payload can carry a secret a surface submitted; clear the
            // Rust copy before it is freed.
            wipe(&mut payload);
            Ok(env.new_string(response)?.into())
        })
        .resolve::<LogErrorAndDefault>()
}

/// `fun nativeReadAsset(path: String): ByteArray?` (framed, see `decodeAsset`).
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_norishell_android_bridge_NoriShellNative_nativeReadAsset<
    'caller,
>(
    mut unowned: EnvUnowned<'caller>,
    _this: JObject<'caller>,
    path: JString<'caller>,
) -> JObject<'caller> {
    unowned
        .with_env(|env| -> Result<JObject<'caller>, jni::errors::Error> {
            let path = path.try_to_string(env)?;
            let Some(asset) = ASSET_READER.get().and_then(|read| read(&path)) else {
                return Ok(JObject::null());
            };
            Ok(env.byte_array_from_slice(&encode_asset(&asset))?.into())
        })
        .resolve::<LogErrorAndDefault>()
}

/// `fun nativeIsolatedDocument(label: String, path: String): ByteArray?`
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_norishell_android_bridge_NoriShellNative_nativeIsolatedDocument<
    'caller,
>(
    mut unowned: EnvUnowned<'caller>,
    _this: JObject<'caller>,
    label: JString<'caller>,
    path: JString<'caller>,
) -> JObject<'caller> {
    unowned
        .with_env(|env| -> Result<JObject<'caller>, jni::errors::Error> {
            let label = label.try_to_string(env)?;
            let path = path.try_to_string(env)?;
            let Some(asset) = ISOLATED_DOCUMENT.get().and_then(|read| read(&label, &path)) else {
                return Ok(JObject::null());
            };
            Ok(env.byte_array_from_slice(&encode_asset(&asset))?.into())
        })
        .resolve::<LogErrorAndDefault>()
}

/// `fun nativeSurfaceDestroyed(label: String)`
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_norishell_android_bridge_NoriShellNative_nativeSurfaceDestroyed<
    'caller,
>(
    mut unowned: EnvUnowned<'caller>,
    _this: JObject<'caller>,
    label: JString<'caller>,
) {
    unowned
        .with_env(|env| -> Result<(), jni::errors::Error> {
            let label = label.try_to_string(env)?;
            if let Some(callback) = SURFACE_DESTROYED.get() {
                callback(&label);
            }
            Ok(())
        })
        .resolve::<LogErrorAndDefault>()
}

/// `fun nativeNotificationAction(action: String)`
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_norishell_android_bridge_NoriShellNative_nativeNotificationAction<
    'caller,
>(
    mut unowned: EnvUnowned<'caller>,
    _this: JObject<'caller>,
    action: JString<'caller>,
) {
    unowned
        .with_env(|env| -> Result<(), jni::errors::Error> {
            let action = action.try_to_string(env)?;
            match NOTIFICATION_ACTION.get() {
                Some(callback) => callback(&action),
                None => crate::log::warn("notification action arrived before registration"),
            }
            Ok(())
        })
        .resolve::<LogErrorAndDefault>()
}

/// `fun nativeNotificationEvent(event: String, id: String)`
///
/// - `foreground` / `background`: whether any app Activity is resumed. Only an
///   atomic store, so Kotlin may call it on the UI thread.
/// - `click`: the user tapped resource notification `id`; forwarded to the
///   registered click callback. Kotlin calls this off the UI thread.
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_norishell_android_bridge_NoriShellNative_nativeNotificationEvent<
    'caller,
>(
    mut unowned: EnvUnowned<'caller>,
    _this: JObject<'caller>,
    event: JString<'caller>,
    id: JString<'caller>,
) {
    unowned
        .with_env(|env| -> Result<(), jni::errors::Error> {
            let event = event.try_to_string(env)?;
            match event.as_str() {
                "foreground" => ACTIVITY_RESUMED.store(true, Ordering::Release),
                "background" => ACTIVITY_RESUMED.store(false, Ordering::Release),
                "click" => {
                    let id = id.try_to_string(env)?;
                    match NOTIFICATION_CLICK.get() {
                        Some(callback) => callback(&id),
                        None => crate::log::warn("notification click arrived before registration"),
                    }
                }
                _ => crate::log::warn("unknown notification event ignored"),
            }
            Ok(())
        })
        .resolve::<LogErrorAndDefault>()
}

/// Overwrites a string's bytes with zeros (still valid UTF-8) through volatile
/// writes so the clear is not optimized away.
fn wipe(text: &mut str) {
    // SAFETY: only NUL bytes are written, which keeps the buffer valid UTF-8.
    for byte in unsafe { text.as_bytes_mut() } {
        // SAFETY: `byte` is a valid, aligned, exclusive reference into the buffer.
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
}
