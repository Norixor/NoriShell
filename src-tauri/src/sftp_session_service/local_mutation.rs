//! File actions in a local pane: create a folder or an empty file, rename,
//! and delete, all inside one registered local directory capability.
//!
//! The capability handle is the only authority: names are single components,
//! existing entries are fenced by the identity seen in their listing, nothing
//! is replaced, and symbolic links are never followed. Objects that an
//! unfinished transfer reads or writes are refused with `sftp.conflict`.

use std::collections::BTreeMap;

use norishell_core_api as wire;
use tauri::{State, Webview};

use super::{
    CoreResult, LifecycleState, SftpRuntimeError, SftpSessionService, TransferState,
    local_capability::{LocalBusySet, LocalDirectoryOwner, LocalMutationFailure},
    new_local_directory_owner,
};
#[cfg(any(unix, windows))]
use super::{LocalDirectoryCapabilityHandle, MUTATION_LEDGER_CAPACITY};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LocalFileMutationError {
    Runtime(SftpRuntimeError),
    Failure(LocalMutationFailure),
}

impl From<SftpRuntimeError> for LocalFileMutationError {
    fn from(error: SftpRuntimeError) -> Self {
        Self::Runtime(error)
    }
}

impl From<LocalMutationFailure> for LocalFileMutationError {
    fn from(failure: LocalMutationFailure) -> Self {
        Self::Failure(failure)
    }
}

/// One replayable outcome per operation id; a duplicate in flight is refused.
#[derive(Clone)]
pub(super) struct LocalMutationLedgerRecord {
    idempotency_key: String,
    fingerprint: Vec<u8>,
    outcome: Option<Result<wire::SftpLocalFileMutationResult, LocalFileMutationError>>,
}

pub(super) type LocalMutationLedger = BTreeMap<String, LocalMutationLedgerRecord>;

fn local_mutation_fingerprint(request: &wire::SftpLocalFileMutationRequest) -> Vec<u8> {
    let mut value = b"sftp-local-file-mutation-v1".to_vec();
    super::fingerprint_field(&mut value, request.directory_ref.as_bytes());
    value.extend_from_slice(&request.expected_revision.get().to_be_bytes());
    let encoded = serde_json::to_vec(&request.mutation).unwrap_or_default();
    super::fingerprint_field(&mut value, &encoded);
    value
}

/// A name for a new or renamed entry: one component, no separators.
fn valid_new_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= 255
        && !name.contains(['/', '\\', '\0'])
}

impl SftpSessionService {
    /// Local objects of unfinished transfers (queued, running, paused,
    /// committing or cancelling). Finished and failed transfers do not block.
    async fn local_busy_set(&self) -> LocalBusySet {
        #[cfg(unix)]
        let active = {
            let records = self.records.lock().await;
            records
                .values()
                .flat_map(|record| {
                    record
                        .actor
                        .transfers()
                        .filter(|transfer| {
                            !matches!(
                                transfer.state,
                                TransferState::Completed
                                    | TransferState::Cancelled
                                    | TransferState::Failed
                            )
                        })
                        .map(|transfer| transfer.transfer_id.clone())
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        };
        let mut busy = LocalBusySet::default();
        #[cfg(unix)]
        {
            let boundaries = self.transfer_boundaries.lock().await;
            for transfer_id in active {
                if let Some(boundary) = boundaries.get(transfer_id.as_str()) {
                    boundary.add_busy_entries(&transfer_id, &mut busy);
                }
            }
        }
        // Windows: an upload keeps its source open without delete sharing, so
        // the OS itself refuses to rename or delete it while it is read.
        #[cfg(not(unix))]
        let _ = TransferState::Completed;
        busy
    }

    pub(super) async fn mutate_local_file(
        &self,
        request: wire::SftpLocalFileMutationRequest,
        owner: &LocalDirectoryOwner,
    ) -> Result<wire::SftpLocalFileMutationResult, LocalFileMutationError> {
        if request.idempotency_key.trim().is_empty() || request.directory_ref.trim().is_empty() {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        match &request.mutation {
            wire::SftpLocalFileMutation::CreateDirectory { name }
            | wire::SftpLocalFileMutation::CreateEmptyFile { name }
            | wire::SftpLocalFileMutation::Rename { new_name: name, .. }
                if !valid_new_name(name) =>
            {
                return Err(LocalMutationFailure::InvalidName.into());
            }
            wire::SftpLocalFileMutation::Delete {
                irreversible_confirmed: false,
                ..
            } => return Err(SftpRuntimeError::InvalidInput.into()),
            _ => {}
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = owner;
            return Err(LocalMutationFailure::Unsupported.into());
        }
        #[cfg(any(unix, windows))]
        {
            let operation_id = request.operation_id.to_string();
            let fingerprint = local_mutation_fingerprint(&request);
            {
                let mut ledger = self.local_mutation_ledger.lock().await;
                if let Some(replay) = ledger.get(&operation_id) {
                    if replay.idempotency_key != request.idempotency_key
                        || replay.fingerprint != fingerprint
                    {
                        return Err(SftpRuntimeError::Conflict.into());
                    }
                    // An identical retry gets the recorded outcome; one that
                    // races the first execution is refused, never re-run.
                    return replay
                        .outcome
                        .clone()
                        .unwrap_or(Err(SftpRuntimeError::Conflict.into()));
                }
                if ledger.len() >= MUTATION_LEDGER_CAPACITY
                    && let Some(oldest) = ledger
                        .iter()
                        .find(|(_, record)| record.outcome.is_some())
                        .map(|(key, _)| key.clone())
                {
                    ledger.remove(&oldest);
                }
                ledger.insert(
                    operation_id.clone(),
                    LocalMutationLedgerRecord {
                        idempotency_key: request.idempotency_key.clone(),
                        fingerprint,
                        outcome: None,
                    },
                );
            }
            let outcome = self.execute_local_mutation(&request, owner).await;
            if let Some(record) = self
                .local_mutation_ledger
                .lock()
                .await
                .get_mut(&operation_id)
            {
                record.outcome = Some(outcome.clone());
            }
            outcome
        }
    }

    #[cfg(any(unix, windows))]
    async fn execute_local_mutation(
        &self,
        request: &wire::SftpLocalFileMutationRequest,
        owner: &LocalDirectoryOwner,
    ) -> Result<wire::SftpLocalFileMutationResult, LocalFileMutationError> {
        let directory = self
            .require_local_directory(&request.directory_ref, request.expected_revision.get())
            .await
            .map_err(|_| SftpRuntimeError::Conflict)?;
        if !directory.owner.admits(owner) {
            return Err(SftpRuntimeError::Conflict.into());
        }
        let entry = match &request.mutation {
            wire::SftpLocalFileMutation::Rename { entry_ref, .. }
            | wire::SftpLocalFileMutation::Delete { entry_ref, .. } => {
                let entry = self
                    .local_directory_entries
                    .lock()
                    .await
                    .get(entry_ref)
                    .cloned()
                    .ok_or(SftpRuntimeError::InvalidInput)?;
                if entry.directory_ref != request.directory_ref
                    || entry.directory_revision != request.expected_revision.get()
                    || entry.expires_at <= std::time::Instant::now()
                {
                    return Err(LocalMutationFailure::PreconditionChanged.into());
                }
                Some(entry)
            }
            _ => None,
        };
        let busy = self.local_busy_set().await;
        let handle = directory.capability.clone();
        let mutation = request.mutation.clone();
        // A timed-out action may still complete; the caller refreshes the
        // listing instead of retrying blindly.
        let result = self
            .run_local_directory_io(SftpRuntimeError::LocalDirectoryTimeout, move || {
                let LocalDirectoryCapabilityHandle::Native(directory) = handle.as_ref();
                Ok::<_, ()>(match (&mutation, entry) {
                    (wire::SftpLocalFileMutation::CreateDirectory { name }, _) => {
                        directory.create_directory_entry(name.as_bytes(), &busy)
                    }
                    (wire::SftpLocalFileMutation::CreateEmptyFile { name }, _) => {
                        directory.create_empty_file(name.as_bytes(), &busy)
                    }
                    (wire::SftpLocalFileMutation::Rename { new_name, .. }, Some(entry)) => {
                        directory.rename_entry(
                            &entry.name,
                            &entry.identity,
                            new_name.as_bytes(),
                            &busy,
                        )
                    }
                    (wire::SftpLocalFileMutation::Delete { recursive, .. }, Some(entry)) => {
                        directory.delete_entry(&entry.name, &entry.identity, *recursive, &busy)
                    }
                    _ => Err(LocalMutationFailure::InvalidName),
                })
            })
            .await?;
        result?;
        Ok(wire::SftpLocalFileMutationResult {
            directory_ref: request.directory_ref.clone(),
            revision: request.expected_revision,
        })
    }
}

fn map_local_mutation_error(
    request_id: wire::RequestId,
    error: LocalFileMutationError,
) -> Box<wire::CoreApiError> {
    let (code, category, retry_strategy) = match error {
        LocalFileMutationError::Failure(failure) => match failure {
            LocalMutationFailure::InvalidName => (
                "sftp.local_invalid_name",
                wire::ErrorCategory::Validation,
                wire::RetryStrategy::Never,
            ),
            LocalMutationFailure::TargetExists => (
                "sftp.local_target_exists",
                wire::ErrorCategory::Conflict,
                wire::RetryStrategy::Never,
            ),
            LocalMutationFailure::PreconditionChanged => (
                "sftp.local_entry_changed",
                wire::ErrorCategory::Conflict,
                wire::RetryStrategy::RefreshSnapshot,
            ),
            LocalMutationFailure::DirectoryNotEmpty => (
                "sftp.local_directory_not_empty",
                wire::ErrorCategory::Conflict,
                wire::RetryStrategy::Never,
            ),
            LocalMutationFailure::InUseByTransfer => (
                "sftp.conflict",
                wire::ErrorCategory::Conflict,
                wire::RetryStrategy::WaitForUser,
            ),
            LocalMutationFailure::PermissionDenied => (
                "sftp.local_permission_denied",
                wire::ErrorCategory::Permission,
                wire::RetryStrategy::Never,
            ),
            LocalMutationFailure::Unsupported => (
                "sftp.local_mutation_unsupported",
                wire::ErrorCategory::Incompatible,
                wire::RetryStrategy::Never,
            ),
            LocalMutationFailure::LimitExceeded => (
                "sftp.local_delete_limit_exceeded",
                wire::ErrorCategory::Validation,
                wire::RetryStrategy::Never,
            ),
            LocalMutationFailure::Io => (
                "sftp.operation_failed",
                wire::ErrorCategory::Unavailable,
                wire::RetryStrategy::RefreshSnapshot,
            ),
        },
        LocalFileMutationError::Runtime(SftpRuntimeError::InvalidInput) => (
            "sftp.invalid_input",
            wire::ErrorCategory::Validation,
            wire::RetryStrategy::Never,
        ),
        LocalFileMutationError::Runtime(SftpRuntimeError::LocalDirectoryTimeout) => (
            "sftp.local_directory_timeout",
            wire::ErrorCategory::Unavailable,
            wire::RetryStrategy::RefreshSnapshot,
        ),
        LocalFileMutationError::Runtime(SftpRuntimeError::Conflict) => (
            "sftp.local_directory_stale",
            wire::ErrorCategory::Conflict,
            wire::RetryStrategy::RefreshSnapshot,
        ),
        LocalFileMutationError::Runtime(_) => (
            "sftp.operation_failed",
            wire::ErrorCategory::Unavailable,
            wire::RetryStrategy::RefreshSnapshot,
        ),
    };
    Box::new(wire::CoreApiError {
        code: code.to_owned(),
        category,
        retry_strategy,
        message_key: "errors.sftp.operationFailed".to_owned(),
        params: BTreeMap::new(),
        request_id: Some(request_id),
        diagnostic_id: None,
        conflict: None,
    })
}

#[tauri::command]
pub(crate) async fn sftp_local_file_mutate(
    webview: Webview,
    request: wire::SftpLocalFileMutationRequest,
    service: State<'_, SftpSessionService>,
    lifecycle: State<'_, LifecycleState>,
) -> CoreResult<wire::SftpLocalFileMutationResult> {
    let request_id = request.meta.request_id.clone();
    let _creation_permit = lifecycle.acquire_resource_creation(request_id.clone())?;
    let owner = new_local_directory_owner(&webview, &request_id)?;
    service
        .mutate_local_file(request, &owner)
        .await
        .map_err(|error| map_local_mutation_error(request_id, error))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{
        host_service::HostService, ssh_agent_service::SshAgentService,
        transient_credential_service::TransientCredentialService, vault_service::VaultService,
    };

    fn owner() -> LocalDirectoryOwner {
        LocalDirectoryOwner {
            label: "main".to_owned(),
            liveness: None,
        }
    }

    fn meta() -> wire::RequestMeta {
        wire::RequestMeta {
            request_id: wire::RequestId::new(),
        }
    }

    async fn listed(
        service: &SftpSessionService,
        capability: &wire::SftpLocalDirectoryCapability,
    ) -> Vec<wire::SftpLocalDirectoryEntry> {
        service
            .list_local_directory(
                wire::SftpLocalDirectoryListRequest {
                    meta: meta(),
                    operation_id: wire::OperationId::new(),
                    idempotency_key: "list".into(),
                    directory_ref: capability.directory_ref.clone(),
                    expected_revision: capability.revision,
                    cursor: None,
                    page_size: 64,
                },
                &owner(),
            )
            .await
            .unwrap()
            .entries
    }

    fn request(
        capability: &wire::SftpLocalDirectoryCapability,
        mutation: wire::SftpLocalFileMutation,
    ) -> wire::SftpLocalFileMutationRequest {
        wire::SftpLocalFileMutationRequest {
            meta: meta(),
            operation_id: wire::OperationId::new(),
            idempotency_key: "mutate".into(),
            directory_ref: capability.directory_ref.clone(),
            expected_revision: capability.revision,
            mutation,
        }
    }

    #[tokio::test]
    async fn local_file_actions_stay_inside_the_capability_and_replay_by_operation() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let service = SftpSessionService::production(
            HostService::start(state.path()).unwrap(),
            VaultService::start(state.path()),
            TransientCredentialService::default(),
            SshAgentService::default(),
        );
        let capability = service
            .register_local_directory(
                wire::SftpLocalDirectoryRegisterRequest {
                    meta: meta(),
                    selected_path: root.path().to_string_lossy().into_owned(),
                },
                &owner(),
            )
            .await
            .unwrap();
        let create = request(
            &capability,
            wire::SftpLocalFileMutation::CreateDirectory {
                name: "folder".into(),
            },
        );
        service
            .mutate_local_file(create.clone(), &owner())
            .await
            .unwrap();
        // The same operation replays its outcome instead of failing on EEXIST.
        service.mutate_local_file(create, &owner()).await.unwrap();
        assert_eq!(
            service
                .mutate_local_file(
                    request(
                        &capability,
                        wire::SftpLocalFileMutation::CreateDirectory {
                            name: "folder".into(),
                        },
                    ),
                    &owner(),
                )
                .await,
            Err(LocalFileMutationError::Failure(
                LocalMutationFailure::TargetExists
            ))
        );
        assert_eq!(
            service
                .mutate_local_file(
                    request(
                        &capability,
                        wire::SftpLocalFileMutation::CreateEmptyFile {
                            name: "../escape".into(),
                        },
                    ),
                    &owner(),
                )
                .await,
            Err(LocalFileMutationError::Failure(
                LocalMutationFailure::InvalidName
            ))
        );
        std::fs::write(root.path().join("folder/inside.txt"), b"x").unwrap();
        let folder = listed(&service, &capability)
            .await
            .into_iter()
            .find(|entry| entry.display_name == "folder")
            .unwrap();
        assert_eq!(
            service
                .mutate_local_file(
                    request(
                        &capability,
                        wire::SftpLocalFileMutation::Delete {
                            entry_ref: folder.entry_ref.clone(),
                            recursive: true,
                            irreversible_confirmed: false,
                        },
                    ),
                    &owner(),
                )
                .await,
            Err(LocalFileMutationError::Runtime(
                SftpRuntimeError::InvalidInput
            ))
        );
        let foreign = LocalDirectoryOwner {
            label: "other-window".to_owned(),
            liveness: None,
        };
        let delete = wire::SftpLocalFileMutation::Delete {
            entry_ref: folder.entry_ref,
            recursive: true,
            irreversible_confirmed: true,
        };
        assert_eq!(
            service
                .mutate_local_file(request(&capability, delete.clone()), &foreign)
                .await,
            Err(LocalFileMutationError::Runtime(SftpRuntimeError::Conflict))
        );
        assert!(root.path().join("folder/inside.txt").exists());
        service
            .mutate_local_file(request(&capability, delete), &owner())
            .await
            .unwrap();
        assert!(!root.path().join("folder").exists());
    }
}
