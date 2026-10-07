//! Native selection is converted to a one-use capability before any guest receives a handle.

use super::*;
use super::{api::ApiAccessReview, api_invocation::ApiInvocation};
use crate::plugin_api::{
    ResourceOwner,
    files::{PluginFileAccessScope, PluginFileGrant},
};
#[cfg(windows)]
use cap_fs_ext::MetadataExt as _;
#[cfg(unix)]
use cap_std::fs::MetadataExt as _;
use cap_std::fs::{Dir, File, Metadata};
use norishell_core_api::{
    PluginApiErrorCode, PluginApiValue, PluginApprovalOperation, PluginFileAccessRequest,
    PluginFilePickerKind,
};

/// Desktop: the native dialog plugin, bounded by the API's five-minute wait.
#[cfg(not(target_os = "android"))]
async fn choose_selection(
    app: &tauri::AppHandle,
    kind: PluginFilePickerKind,
) -> Result<crate::content_access::Selection, PluginApiErrorCode> {
    let (selected, selection) = tokio::sync::oneshot::channel();
    let dialog = app.dialog().file();
    match kind {
        PluginFilePickerKind::File => dialog.pick_file(move |path| {
            let _ = selected.send(path);
        }),
        #[cfg(desktop)]
        PluginFilePickerKind::Directory => dialog.pick_folder(move |path| {
            let _ = selected.send(path);
        }),
        #[cfg(mobile)]
        PluginFilePickerKind::Directory => {
            drop((dialog, selected));
            return Err(PluginApiErrorCode::Unsupported);
        }
    }
    let selected = tokio::time::timeout(std::time::Duration::from_secs(300), selection)
        .await
        .map_err(|_| PluginApiErrorCode::TimedOut)?
        .map_err(|_| PluginApiErrorCode::Cancelled)?
        .ok_or(PluginApiErrorCode::Cancelled)?;
    crate::content_access::classify_dialog(selected).map_err(|_| PluginApiErrorCode::InvalidRequest)
}

/// Android: Core's own document picker. The dialog plugin returns its result to
/// MainActivity only once MainActivity is started again, which never happens
/// while a plugin-isolated or other Secure Surface covers it; this one returns to
/// whatever is in front, is cancellable and closes itself after a fixed timeout.
/// Directory picks stay unsupported (the document picker yields content URIs).
#[cfg(target_os = "android")]
async fn choose_selection(
    _app: &tauri::AppHandle,
    kind: PluginFilePickerKind,
) -> Result<crate::content_access::Selection, PluginApiErrorCode> {
    use crate::content_access::{ContentAccessError, Selection, pick_document};
    if matches!(kind, PluginFilePickerKind::Directory) {
        return Err(PluginApiErrorCode::Unsupported);
    }
    let picked = tokio::task::spawn_blocking(|| pick_document(&["*/*"]))
        .await
        .map_err(|_| PluginApiErrorCode::Unavailable)?;
    match picked {
        Ok(Some(uri)) => Ok(Selection::Content(uri)),
        Ok(None) => Err(PluginApiErrorCode::Cancelled),
        Err(ContentAccessError::PickBusy) => Err(PluginApiErrorCode::Busy),
        Err(ContentAccessError::PickTimeout) => Err(PluginApiErrorCode::TimedOut),
        Err(ContentAccessError::Rejected) => Err(PluginApiErrorCode::InvalidRequest),
        Err(_) => Err(PluginApiErrorCode::Unavailable),
    }
}

impl PluginService {
    pub(super) async fn pick_api_file(
        &self,
        invocation: &ApiInvocation<'_>,
        installed: &PluginInstalledRecord,
        owner: &ResourceOwner,
        kind: PluginFilePickerKind,
        access: PluginFileAccessRequest,
    ) -> Result<PluginApiValue, PluginApiErrorCode> {
        if !invocation.explicit_user_action() {
            return Err(PluginApiErrorCode::InteractionRequired);
        }
        validate_access(kind, access)?;
        let current = self.api_invocation_fence(owner, invocation);
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        self.has_capability(
            invocation.request_id().clone(),
            &owner.plugin_id,
            &owner.signer,
            PluginCapability::LocalFiles,
        )
        .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        let app = self
            .app_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or(PluginApiErrorCode::Unavailable)?;
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let selection = choose_selection(&app, kind).await?;
        // The approval prompt and the plugin see the provider's display name
        // for Android documents instead of the private snapshot path.
        #[cfg_attr(not(target_os = "android"), allow(unused_mut))]
        let mut display_label: Option<String> = None;
        #[cfg_attr(
            not(target_os = "android"),
            allow(clippy::infallible_destructuring_match)
        )]
        let path = match selection {
            crate::content_access::Selection::Path(path) => path,
            #[cfg(target_os = "android")]
            crate::content_access::Selection::Content(uri) => {
                let (path, label) = android_document_snapshot(&app, uri, access).await?;
                display_label = Some(label);
                path
            }
        };
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let owner = owner.clone();
        let grant_owner = owner.clone();
        let (grant, label, persisted_label, exact_scope) = tokio::task::spawn_blocking(move || {
            // Resolve only the user-selected ambient path. All later operations are relative to
            // the retained directory/file capability, even if its original name is replaced.
            let path = std::fs::canonicalize(path).map_err(|_| PluginApiErrorCode::Unavailable)?;
            let path_text = path.to_str().ok_or(PluginApiErrorCode::InvalidRequest)?.to_owned();
            let opaque_id = uuid::Uuid::new_v4().simple().to_string();
            let selected_kind = match kind {
                PluginFilePickerKind::File => "file",
                PluginFilePickerKind::Directory => "directory",
            };
            let persisted_label = format!(
                "Selected local {selected_kind} · {}",
                &opaque_id[..8]
            );
            let scope = PluginFileAccessScope {
                allow_read: access.read, allow_write: access.write, allow_list: access.list,
                allow_rename: access.rename, allow_remove: access.remove,
                allow_recursive_remove: access.recursive_remove, allow_watch: access.watch,
            };
            let (grant, identity) = match kind {
                PluginFilePickerKind::Directory => {
                    let dir = Dir::open_ambient_dir(&path, cap_std::ambient_authority()).map_err(|_| PluginApiErrorCode::Unavailable)?;
                    let identity = file_identity(&dir.dir_metadata().map_err(|_| PluginApiErrorCode::Unavailable)?)?;
                    (PluginFileGrant::directory(grant_owner, dir, scope), identity)
                }
                PluginFilePickerKind::File => {
                    let parent = path.parent().ok_or(PluginApiErrorCode::InvalidRequest)?;
                    let name = path.file_name().ok_or(PluginApiErrorCode::InvalidRequest)?.to_owned();
                    let dir = Dir::open_ambient_dir(parent, cap_std::ambient_authority()).map_err(|_| PluginApiErrorCode::Unavailable)?;
                    let file: File = dir.open(&name).map_err(|_| PluginApiErrorCode::Unavailable)?;
                    let identity = file_identity(&file.metadata().map_err(|_| PluginApiErrorCode::Unavailable)?)?;
                    (PluginFileGrant::selected_file(grant_owner, dir, name, file, scope)?, identity)
                }
            };
            Ok::<_, PluginApiErrorCode>((
                grant,
                path_text.clone(),
                persisted_label,
                serde_json::json!({"path": path_text, "identity": identity, "pickerKind": kind, "access": access}),
            ))
        }).await.map_err(|_| PluginApiErrorCode::Unavailable)??;
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let label = display_label.unwrap_or(label);
        let details = serde_json::to_string_pretty(&exact_scope)
            .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
        let fence = self
            .authorize_api_access(
                invocation,
                installed,
                &owner,
                ApiAccessReview {
                    capability: PluginCapability::LocalFiles,
                    operation: PluginApprovalOperation::FileAccess,
                    target_label: label.clone(),
                    persisted_target_label: persisted_label,
                    details,
                    exact_scope,
                    policy_identity: Some("file.access.v1"),
                    target_fence: None,
                },
            )
            .await?;
        let _permit = self
            .api_creation_permit(invocation.request_id().clone())
            .map_err(|_| PluginApiErrorCode::Revoked)?;
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let files = invocation.resource_consumer().map_or_else(
            || self.api.files.clone(),
            |consumer| self.api.files.for_consumer(consumer),
        );
        let root_handle = files.register(grant, fence).await?;
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        Ok(PluginApiValue::FilePicked { root_handle, label })
    }
}

/// Largest Android document a plugin may receive as a read-only snapshot.
#[cfg(target_os = "android")]
const MAX_ANDROID_PLUGIN_FILE_SNAPSHOT_BYTES: u64 = 64 * 1024 * 1024;

/// Android documents are not filesystem objects. A plugin that only reads the
/// selected file gets a private read-only snapshot; any write, rename, remove
/// or watch access would silently diverge from the user's document, so it is
/// refused with `Unsupported`. The snapshot directory stays until the
/// process-start wipe or the exit cleanup because file grants have no drop hook.
#[cfg(target_os = "android")]
async fn android_document_snapshot(
    app: &tauri::AppHandle,
    uri: crate::content_access::ContentUri,
    access: PluginFileAccessRequest,
) -> Result<(std::path::PathBuf, String), PluginApiErrorCode> {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

    if access.write
        || access.rename
        || access.remove
        || access.recursive_remove
        || access.watch
        || access.list
    {
        return Err(PluginApiErrorCode::Unsupported);
    }
    let app = app.clone();
    tokio::task::spawn_blocking(move || {
        let mut opened = uri
            .open_regular(&app, MAX_ANDROID_PLUGIN_FILE_SNAPSHOT_BYTES)
            .map_err(|error| match error {
                crate::content_access::ContentAccessError::TooLarge => {
                    PluginApiErrorCode::QuotaExceeded
                }
                crate::content_access::ContentAccessError::PermissionDenied
                | crate::content_access::ContentAccessError::Rejected => {
                    PluginApiErrorCode::PermissionDenied
                }
                _ => PluginApiErrorCode::Unavailable,
            })?;
        let name =
            crate::content_access::safe_file_name(opened.display_name.as_deref(), "selected-file");
        let directory = crate::content_access::PrivateDir::create(&app, "plugin-file")
            .map_err(|_| PluginApiErrorCode::Unavailable)?;
        let path = directory.path().join(&name);
        let mut target = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
            .map_err(|_| PluginApiErrorCode::Unavailable)?;
        let expected = opened.len.ok_or(PluginApiErrorCode::Unavailable)?;
        let copied = std::io::copy(
            &mut std::io::Read::take(&mut opened.file, MAX_ANDROID_PLUGIN_FILE_SNAPSHOT_BYTES + 1),
            &mut target,
        )
        .map_err(|_| PluginApiErrorCode::Unavailable)?;
        if copied != expected {
            return Err(PluginApiErrorCode::Unavailable);
        }
        target
            .flush()
            .and_then(|()| target.sync_all())
            .and_then(|()| target.set_permissions(std::fs::Permissions::from_mode(0o400)))
            .map_err(|_| PluginApiErrorCode::Unavailable)?;
        drop(target);
        directory.keep_until_exit();
        Ok((path, name))
    })
    .await
    .map_err(|_| PluginApiErrorCode::Unavailable)?
}

fn validate_access(
    kind: PluginFilePickerKind,
    access: PluginFileAccessRequest,
) -> Result<(), PluginApiErrorCode> {
    if !(access.read
        || access.write
        || access.list
        || access.rename
        || access.remove
        || access.watch)
        || (access.recursive_remove && (!access.remove || kind != PluginFilePickerKind::Directory))
        || (access.list && kind != PluginFilePickerKind::Directory)
    {
        Err(PluginApiErrorCode::InvalidRequest)
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn file_identity(metadata: &Metadata) -> Result<String, PluginApiErrorCode> {
    Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn file_identity(metadata: &Metadata) -> Result<String, PluginApiErrorCode> {
    Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
}
