use std::{
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread::ThreadId,
};

use serde::{Serialize, de::DeserializeOwned};
use tauri::{
    Runtime,
    plugin::{Builder, PluginHandle, TauriPlugin},
};

/// Kotlin package and class of the bridge plugin.
const PLUGIN_IDENTIFIER: &str = "app.norishell.android.bridge";
const PLUGIN_CLASS: &str = "NoriShellPlugin";

type Invoker =
    dyn Fn(&str, serde_json::Value) -> Result<serde_json::Value, BridgeError> + Send + Sync;

static INVOKER: OnceLock<Box<Invoker>> = OnceLock::new();
/// The Android UI thread, recorded by `nativeInit` from the plugin constructor.
/// Kotlin commands run on that thread, so a blocking call from it would deadlock.
pub(crate) static UI_THREAD: OnceLock<ThreadId> = OnceLock::new();
pub(crate) static VERIFIER_READY: AtomicBool = AtomicBool::new(false);

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    /// The plugin has not been registered yet (before `Builder::build`).
    #[error("android_bridge.not_ready")]
    NotReady,
    /// A blocking bridge call was attempted on the Android UI thread.
    #[error("android_bridge.ui_thread")]
    UiThread,
    /// The Kotlin command rejected with a stable code.
    #[error("android_bridge.rejected: {code}")]
    Rejected { code: String },
    /// JNI/serialization failure between Rust and Kotlin.
    #[error("android_bridge.transport")]
    Transport,
    /// No Android Activity is attached (it was destroyed or not created yet);
    /// the runtime cannot dispatch to Kotlin.
    #[error("android_bridge.activity_unavailable")]
    ActivityUnavailable,
}

impl BridgeError {
    /// Stable machine code for the failure (Kotlin code when rejected).
    pub fn code(&self) -> &str {
        match self {
            Self::NotReady => "android_bridge.not_ready",
            Self::UiThread => "android_bridge.ui_thread",
            Self::Rejected { code } => code,
            Self::Transport => "android_bridge.transport",
            Self::ActivityUnavailable => "android_bridge.activity_unavailable",
        }
    }
}

/// Registers the Kotlin `NoriShellPlugin` and keeps its handle for
/// [`run_command`]. The plugin exposes no WebView commands: its invoke
/// handler rejects everything, so the main WebView can never reach Kotlin
/// commands even if an ACL entry were added by mistake.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("norishell-android")
        .invoke_handler(|invoke| {
            invoke.resolver.reject("android_bridge.command_not_exposed");
            true
        })
        .setup(|_app, api| {
            let handle: PluginHandle<R> =
                api.register_android_plugin(PLUGIN_IDENTIFIER, PLUGIN_CLASS)?;
            let invoker = move |command: &str, payload: serde_json::Value| {
                handle
                    .run_mobile_plugin::<serde_json::Value>(command, payload)
                    .map_err(map_invoke_error)
            };
            if INVOKER.set(Box::new(invoker)).is_err() {
                crate::log::warn("bridge plugin registered twice; keeping the first handle");
            }
            crate::log::info("bridge plugin registered");
            Ok(())
        })
        .build()
}

fn map_invoke_error(error: tauri::plugin::mobile::PluginInvokeError) -> BridgeError {
    match error {
        tauri::plugin::mobile::PluginInvokeError::InvokeRejected(response) => {
            BridgeError::Rejected {
                code: response
                    .code
                    .unwrap_or_else(|| "android_bridge.rejected".to_owned()),
            }
        }
        _ => BridgeError::Transport,
    }
}

/// True once the Kotlin plugin is registered and commands can be sent.
pub fn is_ready() -> bool {
    INVOKER.get().is_some()
}

/// True once `rustls-platform-verifier` received the JVM and app context.
pub fn verifier_initialized() -> bool {
    VERIFIER_READY.load(Ordering::Acquire)
}

/// Runs a Kotlin `@Command` of `NoriShellPlugin` and blocks for its result.
///
/// Must not be called from the Android UI thread (returns
/// [`BridgeError::UiThread`]); Kotlin->Rust callbacks that need Kotlin again
/// must hop to another thread first.
pub fn run_command<T: DeserializeOwned>(
    command: &str,
    payload: impl Serialize,
) -> Result<T, BridgeError> {
    if UI_THREAD
        .get()
        .is_some_and(|ui| *ui == std::thread::current().id())
    {
        return Err(BridgeError::UiThread);
    }
    let invoker = INVOKER.get().ok_or(BridgeError::NotReady)?;
    let payload = serde_json::to_value(payload).map_err(|_| BridgeError::Transport)?;
    // The runtime panics when no Activity is attached. Contain it here: many
    // callers run on JNI/FFI threads where an unwinding panic would abort.
    let response =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| invoker(command, payload)))
            .map_err(|_| BridgeError::ActivityUnavailable)??;
    serde_json::from_value(response).map_err(|_| BridgeError::Transport)
}
