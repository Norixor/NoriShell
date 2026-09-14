//! Protocol execution for a Core-owned server-to-server SFTP copy.
//!
//! Session scheduling, endpoint fencing and lifecycle ownership remain in the
//! parent service. This module owns the byte path and target commit boundary so
//! file contents never cross into the renderer or an unbounded queue.

use super::*;

pub(super) async fn execute(
    service: &SftpSessionService,
    plan: &RemoteCopyPlan,
    source_live: &SftpProductionSession<TrustedSftpHostKeyVerifier>,
    target_live: &SftpProductionSession<TrustedSftpHostKeyVerifier>,
) -> Result<RemoteCopyExecution, RemoteCopyExecutionFailure> {
    if source_live.generation != plan.source_generation
        || target_live.generation != plan.target_generation
    {
        return Err(remote_copy_failure(TransferFailureCode::TransportLost));
    }

    let source_path = remote_path_utf8(&plan.source_path)
        .map_err(|_| remote_copy_failure(TransferFailureCode::UnsupportedPathEncoding))?;
    let target_path = remote_path_utf8(&plan.target_path)
        .map_err(|_| remote_copy_failure(TransferFailureCode::UnsupportedPathEncoding))?;
    let temporary_target = remote_temporary_target(&plan.target_path, &plan.transfer_id)
        .map_err(|_| remote_copy_failure(TransferFailureCode::UnsupportedPathEncoding))?;
    let temporary_path = remote_path_utf8(&temporary_target)
        .map_err(|_| remote_copy_failure(TransferFailureCode::UnsupportedPathEncoding))?;

    let source_metadata = bounded_transfer_io(
        source_live.transport.client().symlink_metadata(source_path),
        TransferFailureCode::Protocol,
    )
    .await
    .map_err(remote_copy_failure)?;
    if !remote_metadata_matches(&source_metadata, &plan.source_precondition)
        || source_metadata.len() != plan.expected_bytes
    {
        return Err(remote_copy_failure(TransferFailureCode::LengthMismatch));
    }

    let target_facts = target_live
        .inspect_transfer_targets(plan.target_generation, &plan.target_path, &temporary_target)
        .await
        .map_err(map_transfer_execution_failure)
        .map_err(remote_copy_failure)?;
    if target_facts.target_existed {
        return Err(remote_copy_failure(match plan.conflict_policy {
            ConflictPolicy::FailIfExists => TransferFailureCode::TargetExists,
            ConflictPolicy::ReplaceSafely => TransferFailureCode::UnsafeReplaceUnsupported,
        }));
    }
    if target_facts.temporary_target_existed {
        let cleanup = target_live
            .cleanup_temporary_target(plan.target_generation, &temporary_target)
            .await
            .map_err(map_transfer_execution_failure)
            .map_err(remote_copy_failure)?;
        if !matches!(cleanup, CleanupOutcome::Cleaned) {
            return Err(failure_with_cleanup(
                plan,
                &temporary_target,
                TransferFailureCode::CleanupIncomplete,
                cleanup,
            ));
        }
    }

    let mut source_file = bounded_transfer_io(
        source_live
            .transport
            .client()
            .open_with_flags(source_path, OpenFlags::READ),
        TransferFailureCode::Protocol,
    )
    .await
    .map_err(remote_copy_failure)?;
    let opened_source_metadata =
        bounded_transfer_io(source_file.metadata(), TransferFailureCode::Protocol)
            .await
            .map_err(remote_copy_failure)?;
    if !remote_metadata_matches(&opened_source_metadata, &plan.source_precondition)
        || opened_source_metadata.len() != plan.expected_bytes
    {
        let _ = source_file.close().await;
        return Err(remote_copy_failure(TransferFailureCode::LengthMismatch));
    }

    let mut target_file = bounded_transfer_io(
        target_live.transport.client().open_with_flags(
            temporary_path,
            OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
        ),
        TransferFailureCode::Protocol,
    )
    .await
    .map_err(remote_copy_failure)?;
    service
        .update_remote_copy_state(plan, wire::SftpTransferState::Transferring)
        .await;

    let copy_result = copy_bounded_stream(
        &mut source_file,
        &mut target_file,
        plan.expected_bytes,
        |copied| service.record_remote_copy_progress(plan, copied),
    )
    .await;
    if let Err(failure) = copy_result {
        let _ = source_file.close().await;
        let _ = target_file.close().await;
        return Err(cleanup_failure(plan, target_live, &temporary_target, failure).await);
    }

    let sync_result =
        bounded_transfer_io(target_file.sync_all(), TransferFailureCode::Protocol).await;
    let target_metadata = if sync_result.is_ok() {
        bounded_transfer_io(target_file.metadata(), TransferFailureCode::Protocol).await
    } else {
        Err(TransferFailureCode::Protocol)
    };
    let source_close =
        bounded_transfer_io(source_file.close(), TransferFailureCode::Protocol).await;
    let target_close =
        bounded_transfer_io(target_file.close(), TransferFailureCode::Protocol).await;
    if source_close.is_err() || target_close.is_err() {
        return Err(cleanup_failure(
            plan,
            target_live,
            &temporary_target,
            TransferFailureCode::Protocol,
        )
        .await);
    }
    match target_metadata {
        Ok(metadata) if metadata.len() == plan.expected_bytes => {}
        Ok(_) => {
            return Err(cleanup_failure(
                plan,
                target_live,
                &temporary_target,
                TransferFailureCode::LengthMismatch,
            )
            .await);
        }
        Err(failure) => {
            return Err(cleanup_failure(plan, target_live, &temporary_target, failure).await);
        }
    }

    service
        .update_remote_copy_state(plan, wire::SftpTransferState::Verifying)
        .await;
    if source_live
        .verify_remote_file(
            plan.source_generation,
            &plan.source_path,
            &plan.source_precondition,
        )
        .await
        .is_err()
    {
        return Err(cleanup_failure(
            plan,
            target_live,
            &temporary_target,
            TransferFailureCode::LengthMismatch,
        )
        .await);
    }

    service
        .update_remote_copy_state(plan, wire::SftpTransferState::Committing)
        .await;
    let commit = tokio::time::timeout(
        SFTP_PROTOCOL_OPERATION_TIMEOUT,
        target_live
            .transport
            .client()
            .hardlink(temporary_path, target_path),
    )
    .await;
    match commit {
        Err(_) => {
            return Err(RemoteCopyExecutionFailure {
                failure_code: wire::SftpTransferFailureCode::Protocol,
                commit_outcome: wire::SftpTransferCommitOutcome::Uncertain,
                cleanup_residual: remote_copy_cleanup_residual(
                    plan,
                    Some(&temporary_target),
                    CleanupOutcome::Residual {
                        opaque_location: safe_remote_path_display(temporary_target.as_bytes()),
                    },
                ),
            });
        }
        Ok(Err(error)) => {
            let failure = match error {
                russh_sftp::client::error::Error::Status(status) => match status.status_code {
                    russh_sftp::protocol::StatusCode::PermissionDenied => {
                        TransferFailureCode::PermissionDenied
                    }
                    russh_sftp::protocol::StatusCode::NoConnection
                    | russh_sftp::protocol::StatusCode::ConnectionLost => {
                        TransferFailureCode::TransportLost
                    }
                    russh_sftp::protocol::StatusCode::OpUnsupported => {
                        TransferFailureCode::UnsafeReplaceUnsupported
                    }
                    _ => TransferFailureCode::TargetExists,
                },
                _ => TransferFailureCode::Protocol,
            };
            return Err(cleanup_failure(plan, target_live, &temporary_target, failure).await);
        }
        Ok(Ok(false)) => {
            return Err(cleanup_failure(
                plan,
                target_live,
                &temporary_target,
                TransferFailureCode::UnsafeReplaceUnsupported,
            )
            .await);
        }
        Ok(Ok(true)) => {}
    }

    let final_metadata = bounded_transfer_io(
        target_live.transport.client().symlink_metadata(target_path),
        TransferFailureCode::Protocol,
    )
    .await;
    if !matches!(final_metadata, Ok(ref metadata) if metadata.is_regular() && metadata.len() == plan.expected_bytes)
    {
        return Err(RemoteCopyExecutionFailure {
            failure_code: wire::SftpTransferFailureCode::LengthMismatch,
            commit_outcome: wire::SftpTransferCommitOutcome::Committed,
            cleanup_residual: remote_copy_cleanup_residual(
                plan,
                Some(&temporary_target),
                CleanupOutcome::Residual {
                    opaque_location: safe_remote_path_display(temporary_target.as_bytes()),
                },
            ),
        });
    }

    let cleanup = target_live
        .cleanup_temporary_target(plan.target_generation, &temporary_target)
        .await
        .unwrap_or_else(|_| CleanupOutcome::Residual {
            opaque_location: safe_remote_path_display(temporary_target.as_bytes()),
        });
    Ok(RemoteCopyExecution {
        cleanup_residual: remote_copy_cleanup_residual(plan, Some(&temporary_target), cleanup),
    })
}

async fn cleanup_failure(
    plan: &RemoteCopyPlan,
    target_live: &SftpProductionSession<TrustedSftpHostKeyVerifier>,
    temporary_target: &RemotePath,
    failure: TransferFailureCode,
) -> RemoteCopyExecutionFailure {
    let cleanup = target_live
        .cleanup_temporary_target(plan.target_generation, temporary_target)
        .await
        .unwrap_or_else(|_| CleanupOutcome::Residual {
            opaque_location: safe_remote_path_display(temporary_target.as_bytes()),
        });
    failure_with_cleanup(plan, temporary_target, failure, cleanup)
}

fn failure_with_cleanup(
    plan: &RemoteCopyPlan,
    temporary_target: &RemotePath,
    failure: TransferFailureCode,
    cleanup: CleanupOutcome,
) -> RemoteCopyExecutionFailure {
    let mut result = remote_copy_failure(failure);
    result.cleanup_residual = remote_copy_cleanup_residual(plan, Some(temporary_target), cleanup);
    result
}

/// Copies one file through a fixed-size buffer. Reads and writes are strictly
/// sequential, so a slow target naturally backpressures the source without an
/// unbounded channel or renderer-owned byte buffer.
pub(super) async fn copy_bounded_stream<R, W, F, Fut>(
    source: &mut R,
    target: &mut W,
    expected_bytes: u64,
    mut progress: F,
) -> Result<u64, TransferFailureCode>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    F: FnMut(u64) -> Fut,
    Fut: Future<Output = ()>,
{
    let mut copied = 0_u64;
    let mut buffer = [0_u8; TRANSFER_CHUNK_BYTES];
    loop {
        let read =
            bounded_transfer_io(source.read(&mut buffer), TransferFailureCode::Protocol).await?;
        if read == 0 {
            break;
        }
        bounded_transfer_io(
            target.write_all(&buffer[..read]),
            TransferFailureCode::Protocol,
        )
        .await?;
        copied = copied.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if copied > expected_bytes {
            return Err(TransferFailureCode::LengthMismatch);
        }
        progress(copied).await;
    }
    if copied != expected_bytes {
        return Err(TransferFailureCode::LengthMismatch);
    }
    Ok(copied)
}
