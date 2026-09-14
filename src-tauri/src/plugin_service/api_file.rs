//! Native selection is converted to a one-use capability before any guest receives a handle.

use super::*;
use super::{api::ApiAccessReview, api_invocation::ApiInvocation};
use crate::plugin_api::{
    ResourceOwner,
    files::{PluginFileAccessScope, PluginFileGrant},
};
use cap_std::fs::{Dir, File, Metadata, MetadataExt as _};
use norishell_core_api::{
    PluginApiErrorCode, PluginApiValue, PluginApprovalOperation, PluginFileAccessRequest,
    PluginFilePickerKind,
};

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
        let (selected, selection) = tokio::sync::oneshot::channel();
        let dialog = app.dialog().file();
        match kind {
            PluginFilePickerKind::File => dialog.pick_file(move |path| {
                let _ = selected.send(path);
            }),
            PluginFilePickerKind::Directory => dialog.pick_folder(move |path| {
                let _ = selected.send(path);
            }),
        }
        let path = tokio::time::timeout(std::time::Duration::from_secs(300), selection)
            .await
            .map_err(|_| PluginApiErrorCode::TimedOut)?
            .map_err(|_| PluginApiErrorCode::Cancelled)?
            .ok_or(PluginApiErrorCode::Cancelled)?
            .into_path()
            .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
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
    Ok(format!(
        "{}:{}",
        metadata
            .volume_serial_number()
            .ok_or(PluginApiErrorCode::Unavailable)?,
        metadata
            .file_index()
            .ok_or(PluginApiErrorCode::Unavailable)?
    ))
}
