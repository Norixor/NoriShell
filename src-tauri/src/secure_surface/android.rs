//! Android Secure Surfaces.
//!
//! A surface is a WebView inside the Kotlin `SecureSurfaceActivity`, served from
//! `https://secure.norishell.localhost/` with bytes read here from the app's
//! embedded assets. The page talks only to Kotlin (`__norishellSecure` web
//! message listener, restricted to that origin); Kotlin binds each message to
//! the surface label it created and forwards whitelisted commands through the
//! bridge JNI dispatcher. Core re-checks command <-> label prefix and that the
//! surface is still alive, then runs the same label-bound handler the desktop
//! command uses. Nothing here passes through the main Tauri WebView.
//!
//! Threading: Kotlin calls the dispatcher from a background executor, so a
//! handler may block on the async runtime and may open nested surfaces (which
//! calls back into Kotlin). Destroy callbacks never run on the UI thread.

use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{Mutex, MutexGuard},
};

use norishell_core_api as wire;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tauri::{AppHandle, Manager as _};
use tauri_plugin_norishell_android as bridge;
use zeroize::Zeroizing;

use super::{SurfaceError, SurfaceHandle, SurfaceKind, SurfaceSpec};

const DENIED: &str = r#"{"ok":false,"error":"secure_surface.denied"}"#;
const INVALID_PAYLOAD: &str = "secure_surface.invalid_payload";
const SERIALIZE_FAILED: &str = "secure_surface.serialize_failed";

type DestroyedCallback = Box<dyn Fn() + Send + Sync>;

/// Non-secret description of a surface, sent to Kotlin on open and re-sent to
/// bring an existing surface forward. Never travels through an Intent.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenPayload {
    label: String,
    kind: SurfaceKind,
    /// Entry document plus exact query and appearance parameters.
    path: String,
    title: String,
    content_protected: bool,
}

#[derive(Serialize)]
struct ClosePayload<'a> {
    label: &'a str,
}

struct LiveSurface {
    open: OpenPayload,
    on_destroyed: Vec<DestroyedCallback>,
}

static SURFACES: Mutex<BTreeMap<String, LiveSurface>> = Mutex::new(BTreeMap::new());

fn surfaces() -> MutexGuard<'static, BTreeMap<String, LiveSurface>> {
    SURFACES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Registers the Kotlin -> Rust callbacks on the first open. Every callback is
/// only reachable through a live surface, so no setup wiring is needed and a
/// missing registration can never leave a surface without its handlers.
fn ensure_registered(app: &AppHandle) {
    static REGISTERED: std::sync::Once = std::sync::Once::new();
    REGISTERED.call_once(|| register(app));
}

fn register(app: &AppHandle) {
    let dispatch_app = app.clone();
    let dispatcher =
        bridge::registry::register_secure_dispatcher(move |label, command, payload| {
            dispatch(&dispatch_app, label, command, payload)
        });
    let asset_app = app.clone();
    let assets = bridge::registry::register_asset_reader(move |path| read_asset(&asset_app, path));
    let document_app = app.clone();
    let documents = bridge::registry::register_isolated_document(move |label, path| {
        isolated_document(&document_app, label, path)
    });
    let destroyed = bridge::registry::register_surface_destroyed(surface_destroyed);
    if dispatcher.is_err() || assets.is_err() || documents.is_err() || destroyed.is_err() {
        bridge::log::warn("secure surface callbacks were already registered");
    }
}

pub(super) fn open(app: &AppHandle, spec: SurfaceSpec) -> Result<SurfaceHandle, SurfaceError> {
    ensure_registered(app);
    let open = OpenPayload {
        label: spec.label.clone(),
        kind: spec.kind,
        path: with_appearance(app, spec.path()),
        title: spec.title.clone(),
        content_protected: spec.kind.content_protected(),
    };
    {
        let mut live = surfaces();
        if live.contains_key(&spec.label) {
            return Err(SurfaceError("secure_surface.label_in_use".to_owned()));
        }
        live.insert(
            spec.label.clone(),
            LiveSurface {
                open: open.clone(),
                on_destroyed: Vec::new(),
            },
        );
    }
    if let Err(code) = send_open(&open) {
        // The surface never existed; nobody registered a destroy callback yet.
        surfaces().remove(&spec.label);
        return Err(SurfaceError(code));
    }
    Ok(SurfaceHandle {
        app: app.clone(),
        label: spec.label,
    })
}

fn send_open(open: &OpenPayload) -> Result<(), String> {
    match bridge::run_command::<Value>("secureSurfaceOpen", open) {
        Ok(_) => Ok(()),
        Err(bridge::BridgeError::UiThread) => {
            // Kotlin commands resolve on the UI thread, so it cannot wait for
            // itself. Open from another thread; a late failure tears the
            // surface down exactly like a user close.
            let open = open.clone();
            std::thread::spawn(move || {
                if bridge::run_command::<Value>("secureSurfaceOpen", &open).is_err() {
                    surface_destroyed(&open.label);
                }
            });
            Ok(())
        }
        Err(error) => Err(error.code().to_owned()),
    }
}

pub(super) fn on_destroyed(_app: &AppHandle, label: &str, callback: DestroyedCallback) {
    let mut live = surfaces();
    match live.get_mut(label) {
        Some(surface) => surface.on_destroyed.push(callback),
        None => {
            drop(live);
            tauri::async_runtime::spawn_blocking(callback);
        }
    }
}

pub(crate) fn is_alive(label: &str) -> bool {
    surfaces().contains_key(label)
}

/// True while any Secure Surface exists. Callers that would show unrelated
/// system UI (for example the notification permission dialog) defer it so it
/// never covers a protected prompt and takes its input.
pub(crate) fn any_open() -> bool {
    !surfaces().is_empty()
}

/// Labels of the live surfaces whose label starts with `prefix`.
pub(crate) fn live_labels(prefix: &str) -> Vec<String> {
    surfaces()
        .keys()
        .filter(|label| label.starts_with(prefix))
        .cloned()
        .collect()
}

/// Brings a live surface forward without an `AppHandle` (generic callers).
pub(crate) fn bring_forward(label: &str) -> Result<(), String> {
    let open = surfaces()
        .get(label)
        .map(|surface| surface.open.clone())
        .ok_or_else(|| "secure_surface.not_alive".to_owned())?;
    send_open(&open)
}

#[derive(Serialize)]
struct PostPayload<'a, T: Serialize> {
    label: &'a str,
    event: &'a str,
    payload: T,
}

/// Delivers a Core event to one live surface page (`listen(event)` there).
/// Kotlin queues it until the page connected. Blocks on the UI thread: call it
/// from a blocking context.
pub(crate) fn post_event<T: Serialize>(label: &str, event: &str, payload: T) -> Result<(), String> {
    if !is_alive(label) {
        return Err("secure_surface.not_alive".to_owned());
    }
    bridge::run_command::<Value>(
        "secureSurfacePost",
        PostPayload {
            label,
            event,
            payload,
        },
    )
    .map(|_| ())
    .map_err(|error| error.code().to_owned())
}

/// Core closes the surface: callbacks run now (off the caller's locks), and
/// Kotlin removes the WebView. Kotlin's own destroyed notification for this
/// label then finds nothing left to run.
pub(super) fn close(_app: &AppHandle, label: &str) {
    let Some(surface) = surfaces().remove(label) else {
        return;
    };
    run_destroyed(surface);
    let label = label.to_owned();
    std::thread::spawn(move || {
        if bridge::run_command::<Value>("secureSurfaceClose", ClosePayload { label: &label })
            .is_err()
        {
            bridge::log::warn("secure surface close was not delivered");
        }
    });
}

pub(super) fn show_existing(_app: &AppHandle, label: &str) -> Option<Result<(), SurfaceError>> {
    let open = surfaces().get(label).map(|surface| surface.open.clone())?;
    Some(send_open(&open).map_err(SurfaceError))
}

/// Kotlin reports that a surface is gone (back key close, page close, system
/// teardown of the Activity). Equivalent to desktop `WindowEvent::Destroyed`.
fn surface_destroyed(label: &str) {
    if let Some(surface) = surfaces().remove(label) {
        run_destroyed(surface);
    }
}

fn run_destroyed(surface: LiveSurface) {
    if surface.on_destroyed.is_empty() {
        return;
    }
    // Callers may hold service locks (a prompt guard closing its surface);
    // the desktop event also arrives later, never inside `destroy()`.
    tauri::async_runtime::spawn_blocking(move || {
        for callback in surface.on_destroyed {
            callback();
        }
    });
}

/// Appearance cannot come from the main WebView's storage (another origin),
/// so Core appends the saved, non-secret Application preferences.
fn with_appearance(app: &AppHandle, path: String) -> String {
    let preferences = app
        .try_state::<crate::host_service::HostService>()
        .and_then(|hosts| {
            hosts
                .with_desktop_repository(|repository| {
                    repository.get_application_preferences(
                        wire::ApplicationPreferenceGroupId::Application,
                    )
                })
                .ok()
        })
        .and_then(|snapshot| snapshot.value);
    let mut parameters = Vec::new();
    if let Some(value) = preferences.as_ref() {
        for (key, field) in [("locale", "locale"), ("theme", "themePreference")] {
            let Some(Value::String(text)) = value.get(field) else {
                continue;
            };
            if !text.is_empty()
                && text.len() <= 16
                && text
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            {
                parameters.push(format!("{key}={text}"));
            }
        }
    }
    if parameters.is_empty() {
        return path;
    }
    let separator = if path.contains('?') { '&' } else { '?' };
    format!("{path}{separator}{}", parameters.join("&"))
}

fn read_asset(app: &AppHandle, path: &str) -> Option<bridge::registry::AssetResponse> {
    let path = path.strip_prefix('/').unwrap_or(path);
    let is_entry = SURFACE_ENTRIES.contains(&path);
    let is_static = path.starts_with("assets/")
        && !path.contains("..")
        && !path.contains('\\')
        && !path.ends_with(".html");
    if !is_entry && !is_static {
        return None;
    }
    let asset = app.asset_resolver().get(path.to_owned())?;
    // A missing static asset falls back to index.html in Tauri's resolver.
    if is_static && asset.mime_type.starts_with("text/html") {
        return None;
    }
    let mut headers = vec![
        ("X-Content-Type-Options".to_owned(), "nosniff".to_owned()),
        ("Referrer-Policy".to_owned(), "no-referrer".to_owned()),
    ];
    if is_entry {
        headers.push(("Cache-Control".to_owned(), "no-store".to_owned()));
    }
    if let Some(csp) = asset.csp_header {
        headers.push(("Content-Security-Policy".to_owned(), csp));
    }
    Some(bridge::registry::AssetResponse {
        status: 200,
        mime_type: asset.mime_type,
        headers,
        body: asset.bytes,
    })
}

const SURFACE_ENTRIES: [&str; 12] = [
    "secure-vault.html",
    "secure-credential.html",
    "secure-ssh-challenge.html",
    "secure-backup.html",
    "secure-desktop.html",
    "secure-plugin-host-approval.html",
    "secure-plugin-permission.html",
    "secure-plugin-remote-approval.html",
    "secure-plugin-terminal-input.html",
    "secure-ssh-sync.html",
    "plugin-isolated.html",
    "tool-window.html",
];

fn isolated_document(
    app: &AppHandle,
    label: &str,
    path: &str,
) -> Option<bridge::registry::AssetResponse> {
    if !label.starts_with(SurfaceKind::PluginIsolated.label_prefix()) || !is_alive(label) {
        return None;
    }
    let service = app.try_state::<crate::plugin_service::PluginService>();
    let body = crate::plugin_service::isolated::document_body(service.as_deref(), label, path)?;
    Some(bridge::registry::AssetResponse {
        status: 200,
        mime_type: "text/html".to_owned(),
        headers: crate::plugin_service::isolated::DOCUMENT_HEADERS
            .iter()
            .filter(|(name, _)| *name != "Content-Type")
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
        body,
    })
}

// ---- Command dispatch -------------------------------------------------------

type Reply = Result<Value, Value>;
type ReplyFuture = Pin<Box<dyn Future<Output = Reply> + Send>>;

fn dispatch(app: &AppHandle, label: &str, command: &str, payload: &str) -> String {
    if !is_alive(label) || !super::command_allowed(command, label) {
        bridge::log::warn("secure surface command denied");
        return DENIED.to_owned();
    }
    let Some(future) = route(app, label, command, payload) else {
        bridge::log::warn("secure surface command has no handler");
        return DENIED.to_owned();
    };
    match tauri::async_runtime::block_on(future) {
        Ok(value) => json!({ "ok": true, "value": value }).to_string(),
        Err(error) => json!({ "ok": false, "error": error }).to_string(),
    }
}

/// Parses the page arguments, then runs `handler` with the Kotlin-bound label.
fn handle<A, F, Fut, T, E>(app: &AppHandle, label: &str, payload: &str, handler: F) -> ReplyFuture
where
    A: DeserializeOwned,
    F: FnOnce(AppHandle, String, A) -> Fut,
    Fut: Future<Output = Result<T, E>> + Send + 'static,
    T: Serialize,
    E: Serialize,
{
    let Ok(arguments) = serde_json::from_str::<A>(payload) else {
        return Box::pin(std::future::ready(Err(json!(INVALID_PAYLOAD))));
    };
    let future = handler(app.clone(), label.to_owned(), arguments);
    Box::pin(async move {
        match future.await {
            Ok(value) => serde_json::to_value(value).map_err(|_| json!(SERIALIZE_FAILED)),
            Err(error) => {
                Err(serde_json::to_value(error).unwrap_or_else(|_| json!(SERIALIZE_FAILED)))
            }
        }
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExitReplyArgs {
    attempt_id: String,
    approved: bool,
}

#[derive(Deserialize)]
struct IdArgs {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PasswordArgs {
    id: String,
    password: String,
    password_confirmation: String,
    confirmed: bool,
}

#[derive(Deserialize)]
struct ResetArgs {
    id: String,
    phrase: String,
    confirmed: bool,
}

#[derive(Deserialize)]
struct CredentialArgs {
    id: String,
    secret: String,
    passphrase: String,
}

#[derive(Deserialize)]
struct ChallengeArgs {
    id: String,
    approved: bool,
    answers: Vec<String>,
}

#[derive(Deserialize)]
struct VaultOpenArgs {
    mode: crate::secure_vault_service::SecureVaultMode,
}

#[derive(Deserialize)]
struct DecisionArgs {
    decision: wire::DesktopPromptDecision,
}

#[derive(Deserialize)]
struct RequestArgs<T> {
    request: T,
}

#[derive(Deserialize)]
struct MetaArgs {
    meta: wire::RequestMeta,
}

#[derive(Deserialize)]
struct NoArgs {}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OperationArgs {
    operation_id: wire::OperationId,
}

#[derive(Deserialize)]
struct TailArgs {
    offset: wire::WireSequence,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveArgs {
    text: String,
    meta: wire::RequestMeta,
    operation_id: wire::OperationId,
}

/// Generic Core commands an editor surface may call; they do not depend on the
/// caller window, so the Android route calls the same command function.
macro_rules! core_command {
    ($app:expr, $label:expr, $payload:expr, $request:ty, |$a:ident, $r:ident| $call:expr) => {
        handle(
            $app,
            $label,
            $payload,
            |$a: AppHandle, _label: String, args: RequestArgs<$request>| {
                let $r = args.request;
                async move { $call }
            },
        )
    };
}

fn route(app: &AppHandle, label: &str, command: &str, payload: &str) -> Option<ReplyFuture> {
    use crate::{
        host_service as hosts, offline_backup_service as backup,
        plugin_operations::prompts as remote, plugin_service as plugins,
        secure_credential_service as credential, secure_ssh_challenge as challenge,
        secure_vault_service as vault, tool_windows as tools,
    };
    Some(match command {
        // Vault prompt
        "secure_vault_get" => handle(app, label, payload, |app, label, a: IdArgs| async move {
            vault::secure_vault_get_for_label(
                &label,
                a.id,
                &app.state::<vault::SecureVaultService>(),
            )
        }),
        "secure_vault_submit" => handle(
            app,
            label,
            payload,
            |app, label, a: PasswordArgs| async move {
                vault::secure_vault_submit_for_label(
                    &label,
                    a.id,
                    Zeroizing::new(a.password),
                    Zeroizing::new(a.password_confirmation),
                    a.confirmed,
                    &app.state::<vault::SecureVaultService>(),
                )
                .await
            },
        ),
        "secure_vault_reset" => {
            handle(app, label, payload, |app, label, a: ResetArgs| async move {
                vault::secure_vault_reset_for_label(
                    &label,
                    a.id,
                    a.phrase,
                    a.confirmed,
                    &app,
                    &app.state::<vault::SecureVaultService>(),
                    &app.state::<crate::vault_service::VaultService>(),
                )
            })
        }
        "secure_vault_cancel" => handle(app, label, payload, |app, label, a: IdArgs| async move {
            vault::secure_vault_cancel_for_label(
                &label,
                a.id,
                &app.state::<vault::SecureVaultService>(),
            )
        }),
        "secure_vault_open" => handle(
            app,
            label,
            payload,
            |app, label, a: VaultOpenArgs| async move {
                vault::secure_vault_open_for_surface(a.mode, &label, &app).await
            },
        ),
        // SSH credential prompt
        "secure_credential_get" => {
            handle(app, label, payload, |app, label, a: IdArgs| async move {
                credential::secure_credential_get_for_label(
                    &label,
                    a.id,
                    &app.state::<credential::SecureCredentialService>(),
                )
            })
        }
        "secure_credential_submit" => handle(
            app,
            label,
            payload,
            |app, label, a: CredentialArgs| async move {
                credential::secure_credential_submit_for_label(
                    &label,
                    a.id,
                    Zeroizing::new(a.secret),
                    Zeroizing::new(a.passphrase),
                    &app.state::<credential::SecureCredentialService>(),
                )
                .await
            },
        ),
        "secure_credential_cancel" => {
            handle(app, label, payload, |app, label, a: IdArgs| async move {
                credential::secure_credential_cancel_for_label(
                    &label,
                    a.id,
                    &app.state::<credential::SecureCredentialService>(),
                )
            })
        }
        // SSH host key / keyboard-interactive challenge
        "secure_ssh_challenge_get" => {
            handle(app, label, payload, |app, label, a: IdArgs| async move {
                challenge::secure_ssh_challenge_get_for_label(
                    &label,
                    a.id,
                    &app.state::<challenge::SecureSshChallengeService>(),
                )
            })
        }
        "secure_ssh_challenge_submit" => handle(
            app,
            label,
            payload,
            |app, label, a: ChallengeArgs| async move {
                challenge::secure_ssh_challenge_submit_for_label(
                    &label,
                    a.id,
                    a.approved,
                    a.answers.into_iter().map(Zeroizing::new).collect(),
                    &app.state::<challenge::SecureSshChallengeService>(),
                )
                .await
            },
        ),
        "secure_ssh_challenge_cancel" => {
            handle(app, label, payload, |app, label, a: IdArgs| async move {
                challenge::secure_ssh_challenge_cancel_for_label(
                    &label,
                    a.id,
                    &app.state::<challenge::SecureSshChallengeService>(),
                )
            })
        }
        // Offline backup password
        "offline_backup_secure_get" => {
            handle(app, label, payload, |app, label, a: IdArgs| async move {
                backup::offline_backup_secure_get_for_label(
                    &label,
                    a.id,
                    &app.state::<backup::OfflineBackupService>(),
                )
            })
        }
        "offline_backup_secure_submit" => handle(
            app,
            label,
            payload,
            |app, label, a: PasswordArgs| async move {
                backup::offline_backup_secure_submit_for_label(
                    &label,
                    a.id,
                    Zeroizing::new(a.password),
                    Zeroizing::new(a.password_confirmation),
                    a.confirmed,
                    &app.state::<backup::OfflineBackupService>(),
                )
            },
        ),
        "offline_backup_secure_cancel" => {
            handle(app, label, payload, |app, label, a: IdArgs| async move {
                backup::offline_backup_secure_cancel_for_label(
                    &label,
                    a.id,
                    &app.state::<backup::OfflineBackupService>(),
                )
            })
        }
        // Remote desktop authentication
        "desktop_prompt_get" => handle(app, label, payload, |app, label, a: IdArgs| async move {
            app.state::<crate::desktop_service::DesktopService>()
                .secure_prompt_get(&label, &a.id)
        }),
        "desktop_prompt_decide" => handle(
            app,
            label,
            payload,
            |app, label, a: DecisionArgs| async move {
                app.state::<crate::desktop_service::DesktopService>()
                    .secure_prompt_decide(&label, a.decision)
            },
        ),
        // Plugin approvals
        "plugin_host_approval_get" => handle(
            app,
            label,
            payload,
            |app, label, a: RequestArgs<wire::PluginHostApprovalGetRequest>| async move {
                plugins::plugin_host_approval_get_for_label(
                    &label,
                    a.request,
                    &app.state::<plugins::PluginService>(),
                )
            },
        ),
        "plugin_host_approval_decide" => handle(
            app,
            label,
            payload,
            |app, label, a: RequestArgs<wire::PluginHostApprovalDecisionRequest>| async move {
                plugins::plugin_host_approval_decide_for_label(
                    &label,
                    a.request,
                    &app,
                    &app.state::<plugins::PluginService>(),
                )
            },
        ),
        "plugin_special_permission_get" => handle(
            app,
            label,
            payload,
            |app, label, a: RequestArgs<wire::PluginSpecialPermissionGetRequest>| async move {
                plugins::plugin_special_permission_get_for_label(
                    &label,
                    a.request,
                    &app.state::<plugins::PluginService>(),
                )
            },
        ),
        "plugin_special_permission_decide" => handle(
            app,
            label,
            payload,
            |app, label, a: RequestArgs<wire::PluginSpecialPermissionDecisionRequest>| async move {
                plugins::plugin_special_permission_decide_for_label(
                    &label,
                    a.request,
                    &app,
                    &app.state::<plugins::PluginService>(),
                )
                .await
            },
        ),
        "plugin_remote_approval_get" => handle(
            app,
            label,
            payload,
            |app, label, a: RequestArgs<wire::PluginRemoteApprovalGetRequest>| async move {
                remote::plugin_remote_approval_get_for_label(
                    &label,
                    a.request,
                    &app.state::<crate::plugin_operations::PluginOperationsService>(),
                )
            },
        ),
        "plugin_remote_approval_decide" => handle(
            app,
            label,
            payload,
            |app, label, a: RequestArgs<wire::PluginRemoteApprovalDecisionRequest>| async move {
                remote::plugin_remote_approval_decide_for_label(
                    &label,
                    a.request,
                    &app.state::<crate::plugin_operations::PluginOperationsService>(),
                )
            },
        ),
        "plugin_credential_input_submit" => handle(
            app,
            label,
            payload,
            |app, label, a: RequestArgs<wire::PluginCredentialInputDecisionRequest>| async move {
                remote::plugin_credential_input_submit_for_label(
                    &label,
                    a.request,
                    &app.state::<crate::plugin_operations::PluginOperationsService>(),
                )
            },
        ),
        "plugin_terminal_input_get" => handle(
            app,
            label,
            payload,
            |app, label, a: RequestArgs<wire::PluginTerminalInputGetRequest>| async move {
                plugins::plugin_terminal_input_get_for_label(
                    &label,
                    a.request,
                    &app.state::<plugins::PluginService>(),
                )
            },
        ),
        "plugin_terminal_input_decide" => handle(
            app,
            label,
            payload,
            |app, label, a: RequestArgs<wire::PluginTerminalInputDecisionRequest>| async move {
                plugins::plugin_terminal_input_decide_for_label(
                    &label,
                    a.request,
                    &app,
                    &app.state::<plugins::PluginService>(),
                )
                .await
            },
        ),
        "ssh_sync_secure_prompt_get" => handle(
            app,
            label,
            payload,
            |app, label, a: RequestArgs<wire::SshSyncSecurePromptGetRequest>| async move {
                app.state::<crate::ssh_sync_exchange_local::NoriShellSshSyncLocalAdapter>()
                    .prompt_snapshot(a.request, &label)
            },
        ),
        "ssh_sync_secure_prompt_decide" => handle(
            app,
            label,
            payload,
            |app, label, a: RequestArgs<wire::SshSyncSecureDecisionRequest>| async move {
                app.state::<crate::ssh_sync_exchange_local::NoriShellSshSyncLocalAdapter>()
                    .decide_prompt(a.request, &label)
            },
        ),
        // Plugin isolated surface wrapper
        "plugin_isolated_surface_content" => handle(
            app,
            label,
            payload,
            |app, label, a: RequestArgs<wire::PluginIsolatedSurfaceContentRequest>| async move {
                app.state::<plugins::PluginService>()
                    .isolated_surface_content(a.request, &label)
            },
        ),
        "plugin_isolated_bridge" => handle(
            app,
            label,
            payload,
            |app, label, a: RequestArgs<wire::PluginIsolatedBridgeRequest>| async move {
                let service = app.state::<plugins::PluginService>().inner().clone();
                Box::pin(service.isolated_bridge(&label, a.request)).await
            },
        ),
        // Tool window lifecycle
        "tool_window_get" => handle(app, label, payload, |app, label, _: NoArgs| async move {
            tools::tool_window_get_for_label(&label, &app.state::<tools::ToolWindows>())
        }),
        "tool_window_changed" => handle(app, label, payload, |app, label, _: NoArgs| async move {
            tools::tool_window_changed_for_label(&app, &label, &app.state::<tools::ToolWindows>())
        }),
        "tool_window_close" => handle(app, label, payload, |app, label, _: NoArgs| async move {
            tools::tool_window_close_for_label(&app, &label, &app.state::<tools::ToolWindows>())
        }),
        "tool_window_exit_reply" => handle(
            app,
            label,
            payload,
            |app, label, a: ExitReplyArgs| async move {
                crate::tool_window_exit::tool_window_exit_reply_for_label(
                    &label,
                    &app.state::<tools::ToolWindows>(),
                    &app.state::<crate::tool_window_exit::ToolWindowExit>(),
                    &a.attempt_id,
                    a.approved,
                )
            },
        ),
        "tool_file_preview" => handle(app, label, payload, |app, label, _: NoArgs| async move {
            tools::tool_file_preview_for_label(
                &label,
                &app.state::<tools::ToolWindows>(),
                app.state(),
            )
            .await
        }),
        "tool_file_tail" => handle(app, label, payload, |app, label, a: TailArgs| async move {
            tools::tool_file_tail_for_label(
                &label,
                &app.state::<tools::ToolWindows>(),
                app.state(),
                a.offset,
            )
            .await
        }),
        "tool_file_save" => handle(app, label, payload, |app, label, a: SaveArgs| async move {
            tools::tool_file_save_for_label(
                &label,
                &app.state::<tools::ToolWindows>(),
                app.state(),
                app.state(),
                a.text,
                a.meta,
                a.operation_id,
            )
            .await
        }),
        // Editor commands shared with the main window
        "host_list" => core_command!(
            app,
            label,
            payload,
            wire::HostListRequest,
            |app, request| hosts::host_list(request, app.state())
        ),
        "host_catalog_list" => core_command!(
            app,
            label,
            payload,
            wire::HostCatalogListRequest,
            |app, request| hosts::host_catalog_list(request, app.state())
        ),
        "host_group_list" => core_command!(
            app,
            label,
            payload,
            wire::HostGroupListRequest,
            |app, request| hosts::host_group_list(request, app.state())
        ),
        "host_group_create" => core_command!(
            app,
            label,
            payload,
            wire::HostGroupCreateRequest,
            |app, request| hosts::host_group_create(request, app.state())
        ),
        "host_tag_list" => core_command!(
            app,
            label,
            payload,
            wire::HostTagListRequest,
            |app, request| hosts::host_tag_list(request, app.state())
        ),
        "host_tag_create" => core_command!(
            app,
            label,
            payload,
            wire::HostTagCreateRequest,
            |app, request| hosts::host_tag_create(request, app.state())
        ),
        "identity_list" => core_command!(
            app,
            label,
            payload,
            wire::IdentityListRequest,
            |app, request| hosts::identity_list(request, app.state())
        ),
        "identity_create" => core_command!(
            app,
            label,
            payload,
            wire::IdentityCreateRequest,
            |app, request| hosts::identity_create(request, app.state())
        ),
        "credential_ref_list" => core_command!(
            app,
            label,
            payload,
            wire::CredentialRefListRequest,
            |app, request| hosts::credential_ref_list(request, app.state())
        ),
        "algorithm_policy_catalog_get" => core_command!(
            app,
            label,
            payload,
            wire::AlgorithmPolicyCatalogGetRequest,
            |app, request| {
                let _ = &app;
                hosts::algorithm_policy_catalog_get(request)
            }
        ),
        "host_connection_config_get" => core_command!(
            app,
            label,
            payload,
            wire::HostConnectionConfigGetRequest,
            |app, request| hosts::host_connection_config_get(request, app.state())
        ),
        "host_configured_create" => core_command!(
            app,
            label,
            payload,
            wire::HostConfiguredCreateRequest,
            |app, request| hosts::host_configured_create(request, app.state(), app.state()).await
        ),
        "host_update" => core_command!(
            app,
            label,
            payload,
            wire::HostUpdateRequest,
            |app, request| hosts::host_update(request, app.state(), app.state()).await
        ),
        "host_organization_replace" => core_command!(
            app,
            label,
            payload,
            wire::HostOrganizationReplaceRequest,
            |app, request| hosts::host_organization_replace(request, app.state())
        ),
        "route_plan_replace" => core_command!(
            app,
            label,
            payload,
            wire::RoutePlanReplaceRequest,
            |app, request| hosts::route_plan_replace(request, app.state(), app.state()).await
        ),
        "algorithm_policy_replace" => core_command!(
            app,
            label,
            payload,
            wire::AlgorithmPolicyReplaceRequest,
            |app, request| hosts::algorithm_policy_replace(request, app.state(), app.state()).await
        ),
        "heartbeat_policy_replace" => core_command!(
            app,
            label,
            payload,
            wire::HeartbeatPolicyReplaceRequest,
            |app, request| hosts::heartbeat_policy_replace(request, app.state())
        ),
        "monitoring_policy_replace" => core_command!(
            app,
            label,
            payload,
            wire::MonitoringPolicyReplaceRequest,
            |app, request| hosts::monitoring_policy_replace(request, app.state(), app.state())
                .await
        ),
        "login_automation_replace" => core_command!(
            app,
            label,
            payload,
            wire::LoginAutomationReplaceRequest,
            |app, request| hosts::login_automation_replace(request, app.state())
        ),
        "login_automation_confirm" => core_command!(
            app,
            label,
            payload,
            wire::LoginAutomationConfirmRequest,
            |app, request| hosts::login_automation_confirm(request, app.state())
        ),
        "login_automation_secret_create" => core_command!(
            app,
            label,
            payload,
            wire::LoginAutomationSecretCreateRequest,
            |app, request| hosts::login_automation_secret_create(request, app.state(), app.state())
        ),
        "login_automation_secret_cancel" => core_command!(
            app,
            label,
            payload,
            wire::LoginAutomationSecretCancelRequest,
            |app, request| hosts::login_automation_secret_cancel(request, app.state(), app.state())
        ),
        "host_create_password_stage" => core_command!(
            app,
            label,
            payload,
            wire::HostCreatePasswordStageRequest,
            |app, request| hosts::host_create_password_stage(request, app.state(), app.state())
        ),
        "host_create_password_cancel" => core_command!(
            app,
            label,
            payload,
            wire::HostCreatePasswordCancelRequest,
            |app, request| hosts::host_create_password_cancel(request, app.state(), app.state())
        ),
        "private_key_file_import_release" => handle(
            app,
            label,
            payload,
            |_app, _label, a: OperationArgs| async move {
                hosts::private_key_file_import_release(a.operation_id);
                Ok::<(), ()>(())
            },
        ),
        "private_key_file_import" => core_command!(
            app,
            label,
            payload,
            wire::PrivateKeyFileImportRequest,
            |app, request| hosts::private_key_file_import(
                request,
                app.clone(),
                app.state(),
                app.state(),
                app.state()
            )
            .await
        ),
        "credential_transient_prepare" => core_command!(
            app,
            label,
            payload,
            wire::TransientCredentialPrepareRequest,
            |app, request| crate::transient_credential_service::credential_transient_prepare(
                request,
                app.state()
            )
            .await
        ),
        "ssh_connection_test" => core_command!(
            app,
            label,
            payload,
            wire::SshConnectionTestRequest,
            |app, request| crate::connection_test_service::ssh_connection_test(
                request,
                app.state(),
                app.state(),
                app.state(),
                app.state(),
                app.state()
            )
            .await
        ),
        "vault_status" => core_command!(
            app,
            label,
            payload,
            wire::VaultStatusRequest,
            |app, request| crate::vault_service::vault_status(request, app.state())
        ),
        "desktop_profile_list" => {
            handle(app, label, payload, |app, _label, a: MetaArgs| async move {
                crate::desktop_service::desktop_profile_list(a.meta, app.state())
            })
        }
        "desktop_profile_save" => core_command!(
            app,
            label,
            payload,
            wire::DesktopProfileSaveRequest,
            |app, request| crate::desktop_service::desktop_profile_save(request, app.state())
        ),
        _ => return None,
    })
}
