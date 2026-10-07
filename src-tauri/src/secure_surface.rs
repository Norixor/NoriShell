//! Core-owned Secure Surfaces: protected prompts, plugin approvals, plugin
//! isolated UI and tool editors.
//!
//! Every surface is opened by Core, never by a page, and its label is the only
//! identity Core trusts for the surface's commands.
//!
//! - Desktop: a native `WebviewWindow` with the shared secure frame, exact
//!   navigation guard and denied new windows (unchanged behavior).
//! - Android: a Kotlin `SecureSurfaceActivity` hosting its own plain WebView at
//!   `https://secure.norishell.localhost/`. Commands arrive through the bridge
//!   JNI dispatcher with the Kotlin-bound label; Kotlin filters them per surface
//!   kind and Core re-checks them against the label prefix here.

use std::fmt;

use serde::Serialize;
use tauri::AppHandle;

#[cfg(target_os = "android")]
pub(crate) mod android;
#[cfg(desktop)]
mod desktop;

/// What a surface shows; selects its entry page, label prefix and the command
/// whitelist on Android.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum SurfaceKind {
    Vault,
    Credential,
    SshChallenge,
    Backup,
    DesktopAuth,
    PluginHostApproval,
    PluginPermission,
    PluginRemoteApproval,
    PluginTerminalInput,
    SshSync,
    PluginIsolated,
    ToolHostEditor,
    ToolDesktopEditor,
    ToolSftpFile,
}

impl SurfaceKind {
    /// Label prefix Core uses for every surface of this kind.
    pub(crate) const fn label_prefix(self) -> &'static str {
        match self {
            Self::Vault => "secure-vault-",
            Self::Credential => "secure-credential-",
            Self::SshChallenge => "secure-ssh-challenge-",
            Self::Backup => "secure-backup-",
            Self::DesktopAuth => "desktop-auth-",
            Self::PluginHostApproval => "secure-plugin-host-",
            Self::PluginPermission => "secure-plugin-permission-",
            Self::PluginRemoteApproval => "secure-plugin-remote-",
            Self::PluginTerminalInput => "secure-plugin-input-",
            Self::SshSync => "secure-ssh-sync-",
            Self::PluginIsolated => "plugin-isolated-",
            Self::ToolHostEditor => "tool-hostEditor-",
            Self::ToolDesktopEditor => "tool-desktopEditor-",
            Self::ToolSftpFile => "tool-sftpFile-",
        }
    }

    /// Entry document of the surface inside the app's embedded assets.
    pub(crate) const fn entry(self) -> &'static str {
        match self {
            Self::Vault => "secure-vault.html",
            Self::Credential => "secure-credential.html",
            Self::SshChallenge => "secure-ssh-challenge.html",
            Self::Backup => "secure-backup.html",
            Self::DesktopAuth => "secure-desktop.html",
            Self::PluginHostApproval => "secure-plugin-host-approval.html",
            Self::PluginPermission => "secure-plugin-permission.html",
            Self::PluginRemoteApproval => "secure-plugin-remote-approval.html",
            Self::PluginTerminalInput => "secure-plugin-terminal-input.html",
            Self::SshSync => "secure-ssh-sync.html",
            Self::PluginIsolated => "plugin-isolated.html",
            Self::ToolHostEditor | Self::ToolDesktopEditor | Self::ToolSftpFile => {
                "tool-window.html"
            }
        }
    }

    /// Android blocks screenshots and recents thumbnails of every surface
    /// that can show or accept secrets. Desktop keeps its label-based rule in
    /// `apply_secure_window_frame`.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) const fn content_protected(self) -> bool {
        !matches!(
            self,
            Self::PluginIsolated
                | Self::ToolHostEditor
                | Self::ToolDesktopEditor
                | Self::ToolSftpFile
        )
    }
}

/// Everything Core decides about a surface before it exists.
pub(crate) struct SurfaceSpec {
    pub(crate) kind: SurfaceKind,
    pub(crate) label: String,
    /// Exact query of the entry document (`prompt=<id>` etc.), never secret.
    pub(crate) query: Option<String>,
    pub(crate) title: String,
    /// Desktop inner size; the shared frame default applies when absent.
    pub(crate) size: Option<(f64, f64)>,
    pub(crate) min_size: Option<(f64, f64)>,
    pub(crate) resizable: Option<bool>,
    /// Desktop only: center the window on its monitor.
    pub(crate) center: bool,
    /// Desktop only: allow navigation to exactly the entry document and deny
    /// new windows. Android always denies both.
    pub(crate) navigation_guard: bool,
}

impl SurfaceSpec {
    pub(crate) fn new(kind: SurfaceKind, label: impl Into<String>) -> Self {
        Self {
            kind,
            label: label.into(),
            query: None,
            title: "NoriShell".to_owned(),
            size: None,
            min_size: None,
            resizable: None,
            center: false,
            navigation_guard: false,
        }
    }

    pub(crate) fn query(mut self, query: impl Into<String>) -> Self {
        self.query = Some(query.into());
        self
    }

    pub(crate) fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub(crate) fn size(mut self, width: f64, height: f64) -> Self {
        self.size = Some((width, height));
        self
    }

    pub(crate) fn min_size(mut self, width: f64, height: f64) -> Self {
        self.min_size = Some((width, height));
        self
    }

    pub(crate) fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = Some(resizable);
        self
    }

    pub(crate) fn centered(mut self) -> Self {
        self.center = true;
        self
    }

    pub(crate) fn navigation_guard(mut self) -> Self {
        self.navigation_guard = true;
        self
    }

    /// Relative URL of the entry document including its exact query.
    pub(crate) fn path(&self) -> String {
        match &self.query {
            Some(query) => format!("{}?{query}", self.kind.entry()),
            None => self.kind.entry().to_owned(),
        }
    }
}

/// The surface could not be created; it never existed for its owner.
#[derive(Debug)]
pub(crate) struct SurfaceError(String);

impl fmt::Display for SurfaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A live surface. Dropping the handle does not close the surface.
pub(crate) struct SurfaceHandle {
    #[cfg(desktop)]
    window: tauri::WebviewWindow,
    #[cfg(target_os = "android")]
    app: AppHandle,
    #[cfg(target_os = "android")]
    label: String,
}

impl SurfaceHandle {
    /// Runs `callback` once the surface is gone for any reason: user close,
    /// system teardown or a Core close. Equivalent to desktop
    /// `WindowEvent::Destroyed`.
    pub(crate) fn on_destroyed(&self, callback: impl Fn() + Send + Sync + 'static) {
        #[cfg(desktop)]
        self.window.on_window_event(move |event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                callback();
            }
        });
        #[cfg(target_os = "android")]
        android::on_destroyed(&self.app, &self.label, Box::new(callback));
    }

    /// Desktop: focuses the window once its first frame was revealed.
    /// Android brings the surface forward when it opens, so this is a no-op.
    pub(crate) fn focus(&self) -> Result<(), SurfaceError> {
        #[cfg(desktop)]
        {
            crate::window_first_show::focus_if_revealed(&self.window)
                .map_err(|error| SurfaceError(error.to_string()))
        }
        #[cfg(target_os = "android")]
        {
            Ok(())
        }
    }

    /// Destroys the surface without asking the page.
    pub(crate) fn destroy(&self) {
        #[cfg(desktop)]
        let _ = self.window.destroy();
        #[cfg(target_os = "android")]
        android::close(&self.app, &self.label);
    }
}

/// Opens a new surface. The label must be unused.
pub(crate) fn open(app: &AppHandle, spec: SurfaceSpec) -> Result<SurfaceHandle, SurfaceError> {
    debug_assert!(spec.label.starts_with(spec.kind.label_prefix()));
    #[cfg(desktop)]
    {
        desktop::open(app, spec).map(|window| SurfaceHandle { window })
    }
    #[cfg(target_os = "android")]
    {
        android::open(app, spec)
    }
}

/// True while the surface (or, for an owner check, the ordinary window) with
/// this label exists.
pub(crate) fn exists(app: &AppHandle, label: &str) -> bool {
    use tauri::Manager as _;
    #[cfg(target_os = "android")]
    if android::is_alive(label) {
        return true;
    }
    app.get_webview_window(label).is_some()
}

/// Destroys the surface without a close request.
pub(crate) fn destroy(app: &AppHandle, label: &str) {
    #[cfg(desktop)]
    {
        use tauri::Manager as _;
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.destroy();
        }
    }
    #[cfg(target_os = "android")]
    android::close(app, label);
}

/// Requests a close; on desktop this goes through the window close request.
pub(crate) fn close(app: &AppHandle, label: &str) {
    #[cfg(desktop)]
    {
        use tauri::Manager as _;
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.close();
        }
    }
    #[cfg(target_os = "android")]
    android::close(app, label);
}

/// Brings an existing surface forward. `None` when it does not exist.
pub(crate) fn show_existing(app: &AppHandle, label: &str) -> Option<Result<(), SurfaceError>> {
    #[cfg(desktop)]
    {
        desktop::show_existing(app, label)
    }
    #[cfg(target_os = "android")]
    {
        android::show_existing(app, label)
    }
}

/// Commands a surface may send through the Android dispatcher, keyed by the
/// label prefixes allowed to send them. This mirrors the desktop capability
/// files (`capabilities/secure-*.json`, `tool-*.json`, `plugin-isolated.json`)
/// without the window-chrome permissions, which have no Android equivalent.
/// Tool surfaces answer the application-exit draft check with
/// `tool_window_exit_reply` (see `tool_window_exit`).
#[cfg(any(target_os = "android", test))]
pub(crate) const SURFACE_COMMANDS: &[(SurfaceKind, &[&str])] = &[
    (
        SurfaceKind::Vault,
        &[
            "secure_vault_get",
            "secure_vault_submit",
            "secure_vault_reset",
            "secure_vault_cancel",
        ],
    ),
    (
        SurfaceKind::Credential,
        &[
            "secure_credential_get",
            "secure_credential_submit",
            "secure_credential_cancel",
            "secure_vault_open",
        ],
    ),
    (
        SurfaceKind::SshChallenge,
        &[
            "secure_ssh_challenge_get",
            "secure_ssh_challenge_submit",
            "secure_ssh_challenge_cancel",
        ],
    ),
    (
        SurfaceKind::Backup,
        &[
            "offline_backup_secure_get",
            "offline_backup_secure_submit",
            "offline_backup_secure_cancel",
        ],
    ),
    (
        SurfaceKind::DesktopAuth,
        &["desktop_prompt_get", "desktop_prompt_decide"],
    ),
    (
        SurfaceKind::PluginHostApproval,
        &["plugin_host_approval_get", "plugin_host_approval_decide"],
    ),
    (
        SurfaceKind::PluginPermission,
        &[
            "plugin_special_permission_get",
            "plugin_special_permission_decide",
        ],
    ),
    (
        SurfaceKind::PluginRemoteApproval,
        &[
            "plugin_remote_approval_get",
            "plugin_remote_approval_decide",
            "plugin_credential_input_submit",
        ],
    ),
    (
        SurfaceKind::PluginTerminalInput,
        &["plugin_terminal_input_get", "plugin_terminal_input_decide"],
    ),
    (
        SurfaceKind::SshSync,
        &[
            "ssh_sync_secure_prompt_get",
            "ssh_sync_secure_prompt_decide",
        ],
    ),
    (
        SurfaceKind::PluginIsolated,
        &["plugin_isolated_surface_content", "plugin_isolated_bridge"],
    ),
    (
        SurfaceKind::ToolHostEditor,
        &[
            "tool_window_get",
            "tool_window_close",
            "tool_window_changed",
            "tool_window_exit_reply",
            "host_list",
            "host_catalog_list",
            "host_group_list",
            "host_group_create",
            "host_tag_list",
            "host_tag_create",
            "identity_list",
            "credential_ref_list",
            "algorithm_policy_catalog_get",
            "host_connection_config_get",
            "host_configured_create",
            "host_update",
            "host_organization_replace",
            "route_plan_replace",
            "algorithm_policy_replace",
            "heartbeat_policy_replace",
            "monitoring_policy_replace",
            "login_automation_replace",
            "login_automation_confirm",
            "login_automation_secret_create",
            "login_automation_secret_cancel",
            "host_create_password_stage",
            "host_create_password_cancel",
            "identity_create",
            "private_key_file_import",
            "private_key_file_import_release",
            "credential_transient_prepare",
            "ssh_connection_test",
            "vault_status",
            "secure_vault_open",
        ],
    ),
    (
        SurfaceKind::ToolDesktopEditor,
        &[
            "tool_window_get",
            "tool_window_close",
            "tool_window_changed",
            "tool_window_exit_reply",
            "desktop_profile_list",
            "desktop_profile_save",
            "host_list",
            "identity_list",
            "credential_ref_list",
            "host_create_password_stage",
            "host_create_password_cancel",
            "vault_status",
            "secure_vault_open",
        ],
    ),
    (
        SurfaceKind::ToolSftpFile,
        &[
            "tool_window_get",
            "tool_window_close",
            "tool_window_changed",
            "tool_window_exit_reply",
            "tool_file_preview",
            "tool_file_tail",
            "tool_file_save",
        ],
    ),
];

/// Core's half of the double whitelist: the command must be allowed for the
/// kind whose prefix the (Kotlin-bound) label carries, and the label must be
/// longer than the prefix.
#[cfg(any(target_os = "android", test))]
pub(crate) fn command_allowed(command: &str, label: &str) -> bool {
    SURFACE_COMMANDS.iter().any(|(kind, commands)| {
        label
            .strip_prefix(kind.label_prefix())
            .is_some_and(|rest| !rest.is_empty())
            && commands.contains(&command)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_bound_to_their_label_prefix() {
        assert!(command_allowed("secure_vault_submit", "secure-vault-1"));
        assert!(!command_allowed(
            "secure_vault_submit",
            "secure-credential-1"
        ));
        assert!(!command_allowed("secure_vault_submit", "secure-vault-"));
        assert!(!command_allowed("secure_vault_submit", "main"));
        assert!(command_allowed("secure_vault_open", "secure-credential-1"));
        assert!(command_allowed("secure_vault_open", "tool-hostEditor-1"));
        assert!(!command_allowed("secure_vault_open", "secure-vault-1"));
        assert!(!command_allowed("tool_file_save", "tool-hostEditor-1"));
        assert!(command_allowed("tool_file_save", "tool-sftpFile-1"));
        assert!(command_allowed(
            "tool_window_exit_reply",
            "tool-hostEditor-1"
        ));
        assert!(command_allowed("tool_window_exit_reply", "tool-sftpFile-1"));
        assert!(!command_allowed("tool_window_exit_reply", "secure-vault-1"));
        assert!(!command_allowed("vault_unlock", "tool-hostEditor-1"));
        assert!(!command_allowed(
            "plugin_isolated_bridge",
            "secure-plugin-host-1"
        ));
    }

    #[test]
    fn prefixes_do_not_shadow_each_other() {
        // `secure-plugin-host-` must not admit `secure-plugin-permission-` labels etc.
        let kinds = SURFACE_COMMANDS.iter().map(|(kind, _)| *kind);
        for kind in kinds.clone() {
            for other in kinds.clone().filter(|other| *other != kind) {
                assert!(
                    !other.label_prefix().starts_with(kind.label_prefix()),
                    "{kind:?} prefix shadows {other:?}"
                );
            }
        }
    }

    #[test]
    fn spec_path_keeps_the_exact_query() {
        let spec = SurfaceSpec::new(SurfaceKind::Vault, "secure-vault-x").query("prompt=x");
        assert_eq!(spec.path(), "secure-vault.html?prompt=x");
        let tool = SurfaceSpec::new(SurfaceKind::ToolSftpFile, "tool-sftpFile-x");
        assert_eq!(tool.path(), "tool-window.html");
    }
}
