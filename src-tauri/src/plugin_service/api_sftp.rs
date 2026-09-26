//! Host, remote directory, and read/write scope authorization for independent SFTP connections.

use super::{api::ApiAccessReview, api_invocation::ApiInvocation, *};
use crate::plugin_api::{
    ResourceFence, ResourceOwner,
    sftp::{PluginSftpDriver, PluginSftpGrant, PluginSftpScopeFlags},
};
use norishell_core_api::{PluginApiErrorCode, PluginApiValue, PluginSftpOperation};

impl PluginService {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn open_api_sftp(
        &self,
        invocation: &ApiInvocation<'_>,
        installed: &PluginInstalledRecord,
        owner: &ResourceOwner,
        host_handle: &str,
        root_path: &str,
        write: bool,
    ) -> Result<PluginApiValue, PluginApiErrorCode> {
        validate_root(root_path)?;
        let read_epoch = self
            .capability_grant_epoch_for_record(
                invocation.request_id().clone(),
                installed,
                PluginCapability::SftpRead,
            )
            .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        let read_scope = self.api_resource_fence(
            owner.clone(),
            Some((PluginCapability::SftpRead, read_epoch)),
        );
        let handle = PluginHostHandle::parse(host_handle.to_owned())
            .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
        let (target, host_scope) = self.resolve_api_host(invocation, installed, &handle)?;
        let admission =
            self.api_host_admission_fence(invocation, owner, &target, host_scope.clone());
        let capability = if write {
            PluginCapability::SftpWrite
        } else {
            PluginCapability::SftpRead
        };
        let permission = self.authorize_api_access(invocation, installed, owner, ApiAccessReview {
            capability,
            operation: if write { norishell_core_api::PluginApprovalOperation::SftpWrite } else { norishell_core_api::PluginApprovalOperation::SftpRead },
            target_label: target.label.clone(),
            persisted_target_label: target.label.clone(),
            details: serde_json::to_string_pretty(&serde_json::json!({
                "host": target.label, "endpoint": target.endpoint, "root": root_path,
                "read": true, "write": write,
            })).map_err(|_| PluginApiErrorCode::InvalidRequest)?,
            exact_scope: serde_json::json!({"hostId": target.host_id, "endpoint": target.endpoint, "root": root_path, "write": write}),
            policy_identity: Some(if write { "sftp.write.v1" } else { "sftp.read.v1" }),
            target_fence: Some(admission.clone()),
        }).await?;
        if !admission() || !read_scope() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let resource_fence: ResourceFence =
            Arc::new(move || permission() && host_scope() && read_scope());
        let _permit = self
            .api_creation_permit(invocation.request_id().clone())
            .map_err(|_| PluginApiErrorCode::Revoked)?;
        let result = self
            .api_sftp_driver(invocation)?
            .open(PluginSftpGrant::new(
                owner.clone(),
                target.host_id,
                target.host_revision,
                target.connection_revision,
                root_path.as_bytes().to_vec(),
                PluginSftpScopeFlags::for_access(write),
                admission.clone(),
                resource_fence,
            ))
            .await?;
        if !admission() {
            return Err(PluginApiErrorCode::OutcomeUnknown);
        }
        Ok(PluginApiValue::Sftp { result })
    }

    pub(super) async fn invoke_api_sftp(
        &self,
        invocation: &ApiInvocation<'_>,
        installed: &PluginInstalledRecord,
        owner: &ResourceOwner,
        operation: &PluginSftpOperation,
    ) -> Result<PluginApiValue, PluginApiErrorCode> {
        let epoch = self
            .capability_grant_epoch_for_record(
                invocation.request_id().clone(),
                installed,
                PluginCapability::SftpRead,
            )
            .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        let permission =
            self.api_resource_fence(owner.clone(), Some((PluginCapability::SftpRead, epoch)));
        let current = self.api_invocation_fence(owner, invocation);
        let fence: ResourceFence = Arc::new(move || permission() && current());
        let result = self
            .api_sftp_driver(invocation)?
            .invoke(owner, operation, fence.clone())
            .await?;
        if !fence() {
            return Err(PluginApiErrorCode::OutcomeUnknown);
        }
        Ok(PluginApiValue::Sftp { result })
    }

    fn api_sftp_driver(
        &self,
        invocation: &ApiInvocation<'_>,
    ) -> Result<PluginSftpDriver, PluginApiErrorCode> {
        let mut driver = self
            .api_sftp
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(driver) = driver.as_ref() {
            return Ok(invocation
                .resource_consumer()
                .map_or_else(|| driver.clone(), |consumer| driver.for_consumer(consumer)));
        }
        let app = self
            .app_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or(PluginApiErrorCode::Unavailable)?;
        let sftp = app
            .try_state::<crate::sftp_session_service::SftpSessionService>()
            .ok_or(PluginApiErrorCode::Unavailable)?
            .inner()
            .clone();
        let created = PluginSftpDriver::new(sftp, self.api.resources.clone());
        *driver = Some(created.clone());
        Ok(invocation.resource_consumer().map_or_else(
            || created.clone(),
            |consumer| created.for_consumer(consumer),
        ))
    }
}

fn validate_root(path: &str) -> Result<(), PluginApiErrorCode> {
    if path.is_empty()
        || path.len() > 4096
        || !path.starts_with('/')
        || path.chars().any(char::is_control)
        || path.contains('\\')
        || path.split('/').any(|part| part == "." || part == "..")
    {
        return Err(PluginApiErrorCode::InvalidRequest);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_absolute_and_cannot_hide_traversal_or_control_text() {
        for path in [
            "",
            "relative",
            "/srv/../etc",
            "/srv/./data",
            "/srv\n/etc",
            "/srv\\etc",
        ] {
            assert_eq!(validate_root(path), Err(PluginApiErrorCode::InvalidRequest));
        }
        assert!(validate_root("/").is_ok());
        assert!(validate_root("/srv/plugin-data").is_ok());
    }
}
