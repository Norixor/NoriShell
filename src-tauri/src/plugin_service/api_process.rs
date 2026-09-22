//! Protected local-process admission. The resource driver receives only a frozen Core plan.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use super::{
    PluginCapability, PluginInstalledRecord, PluginService, api::ApiAccessReview,
    api_invocation::ApiInvocation,
};
use crate::plugin_api::{ResourceOwner, process::ProcessDriver};
use norishell_core_api::{
    FrozenPluginProcessPlan, PluginApiErrorCode, PluginApiValue, PluginApprovalOperation,
    PluginProcessSendRequest,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct ProgramIdentity {
    canonical_path: String,
    file_identity: String,
    modified_unix_nanos: u128,
    size_bytes: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
struct PreparedPluginProcess {
    program: PathBuf,
    arguments: Vec<String>,
    home: PathBuf,
    timeout_ms: u32,
    identity: ProgramIdentity,
    #[serde(skip)]
    plan: FrozenPluginProcessPlan,
}

impl PluginService {
    pub(super) async fn start_api_process(
        &self,
        invocation: &ApiInvocation<'_>,
        installed: &PluginInstalledRecord,
        owner: &ResourceOwner,
        program: &str,
        arguments: &[String],
        timeout_ms: u32,
    ) -> Result<PluginApiValue, PluginApiErrorCode> {
        self.has_capability(
            invocation.request_id().clone(),
            &owner.plugin_id,
            &owner.signer,
            PluginCapability::LocalProcess,
        )
        .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        let current = self.api_invocation_fence(owner, invocation);
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let prepared = prepare_process(program.to_owned(), arguments.to_vec(), timeout_ms).await?;
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let details = self.process_approval_details(invocation, installed, owner, &prepared)?;
        let exact_scope =
            serde_json::to_value(&prepared).map_err(|_| PluginApiErrorCode::InvalidRequest)?;
        let target_label = process_target_label(&prepared);
        let resource_fence = self
            .authorize_api_access(
                invocation,
                installed,
                owner,
                ApiAccessReview {
                    capability: PluginCapability::LocalProcess,
                    operation: PluginApprovalOperation::LocalExecute,
                    target_label: target_label.clone(),
                    persisted_target_label: target_label,
                    details,
                    exact_scope,
                    target_fence: None,
                },
            )
            .await?;
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        verify_prepared_process(prepared.clone()).await?;
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let _permit = self
            .api_creation_permit(invocation.request_id().clone())
            .map_err(|_| PluginApiErrorCode::Revoked)?;
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let resources = invocation.resource_registry(&self.api.resources);
        let handle =
            ProcessDriver::start(&resources, owner.clone(), prepared.plan, resource_fence)?;
        if !current() {
            // Once the frozen plan crossed the driver boundary the resource retains its ordinary
            // instance/capability lifetime. The executable may already have started, so this is
            // not a revocable operation and must not be presented as a safe retry.
            return Err(PluginApiErrorCode::OutcomeUnknown);
        }
        Ok(PluginApiValue::ProcessStarted { handle })
    }

    pub(super) async fn send_api_process(
        &self,
        invocation: &ApiInvocation<'_>,
        owner: &ResourceOwner,
        request: &PluginProcessSendRequest,
    ) -> Result<PluginApiValue, PluginApiErrorCode> {
        self.has_capability(
            invocation.request_id().clone(),
            &owner.plugin_id,
            &owner.signer,
            PluginCapability::LocalProcess,
        )
        .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        let installed = self
            .hosts
            .with_plugin_repository(|repository| {
                repository.get_plugin_installation(&owner.plugin_id)
            })
            .map_err(|_| PluginApiErrorCode::Unavailable)?;
        let epoch = self
            .capability_grant_epoch_for_record(
                invocation.request_id().clone(),
                &installed,
                PluginCapability::LocalProcess,
            )
            .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        let current = self.api_invocation_fence(owner, invocation);
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let lifetime =
            self.api_resource_fence(owner.clone(), Some((PluginCapability::LocalProcess, epoch)));
        let resources = invocation.resource_registry(&self.api.resources);
        ProcessDriver::send(&resources, owner, request.clone(), &lifetime).await?;
        if !current() {
            // The Core writer acknowledged bytes, but the program's handling cannot be rolled
            // back after the surface fence changed.
            return Err(PluginApiErrorCode::OutcomeUnknown);
        }
        Ok(PluginApiValue::ProcessSent {
            handle: request.handle.clone(),
        })
    }

    fn process_approval_details(
        &self,
        invocation: &ApiInvocation<'_>,
        installed: &PluginInstalledRecord,
        owner: &ResourceOwner,
        prepared: &PreparedPluginProcess,
    ) -> Result<String, PluginApiErrorCode> {
        let locale = self
            .active_instance(
                invocation.request_id().clone(),
                &installed.plugin_id,
                &installed.signer_fingerprint_sha256,
                owner.generation,
            )
            .map_err(|_| PluginApiErrorCode::Revoked)?
            .locale;
        let scope = serde_json::to_string_pretty(prepared)
            .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
        let warning = if locale.as_str() == "zh-CN" {
            "此本机程序会以当前用户身份运行，可能访问当前用户可访问的文件与网络。清空环境变量不是操作系统沙箱。"
        } else {
            "This local program runs as the current user and may access that user's files and network. Clearing its environment is not an OS sandbox."
        };
        Ok(format!("{warning}\n\n{scope}"))
    }
}

async fn prepare_process(
    program: String,
    arguments: Vec<String>,
    timeout_ms: u32,
) -> Result<PreparedPluginProcess, PluginApiErrorCode> {
    tokio::task::spawn_blocking(move || prepare_process_blocking(&program, arguments, timeout_ms))
        .await
        .map_err(|_| PluginApiErrorCode::Unavailable)?
}

async fn verify_prepared_process(
    prepared: PreparedPluginProcess,
) -> Result<(), PluginApiErrorCode> {
    tokio::task::spawn_blocking(move || {
        let identity = snapshot_program(&prepared.program)?;
        let home = core_user_home()?;
        if identity != prepared.identity || home != prepared.home {
            return Err(PluginApiErrorCode::Conflict);
        }
        Ok(())
    })
    .await
    .map_err(|_| PluginApiErrorCode::Unavailable)?
}

fn prepare_process_blocking(
    program: &str,
    arguments: Vec<String>,
    timeout_ms: u32,
) -> Result<PreparedPluginProcess, PluginApiErrorCode> {
    let source = Path::new(program);
    if !source.is_absolute() {
        return Err(PluginApiErrorCode::InvalidRequest);
    }
    let program = std::fs::canonicalize(source).map_err(|_| PluginApiErrorCode::Unavailable)?;
    let identity = snapshot_program(&program)?;
    let home = core_user_home()?;
    let plan = FrozenPluginProcessPlan::new(
        program.clone(),
        arguments.iter().map(OsString::from).collect(),
        home.clone(),
        timeout_ms,
    )?;
    Ok(PreparedPluginProcess {
        program,
        arguments,
        home,
        timeout_ms,
        identity,
        plan,
    })
}

fn core_user_home() -> Result<PathBuf, PluginApiErrorCode> {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME");
    let home = home.ok_or(PluginApiErrorCode::Unavailable)?;
    let home =
        std::fs::canonicalize(PathBuf::from(home)).map_err(|_| PluginApiErrorCode::Unavailable)?;
    std::fs::metadata(&home)
        .map_err(|_| PluginApiErrorCode::Unavailable)?
        .is_dir()
        .then_some(home)
        .ok_or(PluginApiErrorCode::Unavailable)
}

fn snapshot_program(path: &Path) -> Result<ProgramIdentity, PluginApiErrorCode> {
    #[cfg(windows)]
    let metadata = cap_std::fs::File::open_ambient(path, cap_std::ambient_authority())
        .and_then(|file| file.metadata())
        .map_err(|_| PluginApiErrorCode::Conflict)?;
    #[cfg(not(windows))]
    let metadata = std::fs::metadata(path).map_err(|_| PluginApiErrorCode::Conflict)?;
    if !metadata.is_file() {
        return Err(PluginApiErrorCode::InvalidRequest);
    }
    let modified = metadata
        .modified()
        .map_err(|_| PluginApiErrorCode::Unavailable)?;
    #[cfg(windows)]
    let modified = modified.into_std();
    let modified_unix_nanos = modified
        .duration_since(UNIX_EPOCH)
        .map_err(|_| PluginApiErrorCode::Unavailable)?
        .as_nanos();
    Ok(ProgramIdentity {
        canonical_path: path
            .to_str()
            .ok_or(PluginApiErrorCode::InvalidRequest)?
            .to_owned(),
        file_identity: file_identity(&metadata)?,
        modified_unix_nanos,
        size_bytes: metadata.len(),
    })
}

fn process_target_label(prepared: &PreparedPluginProcess) -> String {
    let name = prepared
        .program
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("local program");
    let mut label = format!("Local process: {name}");
    while label.len() > 512 {
        label.pop();
    }
    label
}

#[cfg(unix)]
fn file_identity(metadata: &std::fs::Metadata) -> Result<String, PluginApiErrorCode> {
    use std::os::unix::fs::MetadataExt as _;
    Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn file_identity(metadata: &cap_std::fs::Metadata) -> Result<String, PluginApiErrorCode> {
    use cap_fs_ext::MetadataExt as _;
    Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
}
