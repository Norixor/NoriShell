use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum IntentCleanupReplayKind {
    Retry,
    RetainForExit,
}

#[derive(Clone)]
pub(super) struct IntentCleanupReplay {
    kind: IntentCleanupReplayKind,
    idempotency_key: String,
    fingerprint: Vec<u8>,
    result: wire::SftpTransferIntentSummary,
}

#[derive(Clone)]
pub(super) struct IntentCleanupExitAuthorization {
    source_fence: wire::SftpTransferEndpointFence,
    target_fence: wire::SftpTransferEndpointFence,
    state_revision: WireSequence,
}

impl IntentCleanupExitAuthorization {
    pub(super) fn matches(&self, summary: &wire::SftpTransferIntentSummary) -> bool {
        self.source_fence == summary.source_fence
            && self.target_fence == summary.target_fence
            && self.state_revision == summary.state_revision
    }
}

fn retry_fingerprint(request: &wire::SftpTransferIntentCleanupRetryRequest) -> Vec<u8> {
    let mut value = b"sftp-transfer-intent-cleanup-retry-v1".to_vec();
    fingerprint_field(&mut value, request.transfer_id.as_str().as_bytes());
    fingerprint_transfer_endpoint_fence(&mut value, &request.expected_source_fence);
    fingerprint_transfer_endpoint_fence(&mut value, &request.expected_target_fence);
    value.extend_from_slice(&request.expected_state_revision.get().to_be_bytes());
    value.extend_from_slice(
        &request
            .expected_target_host_state_version
            .get()
            .to_be_bytes(),
    );
    value
}

fn retain_fingerprint(request: &wire::SftpTransferIntentCleanupRetainRequest) -> Vec<u8> {
    let mut value = b"sftp-transfer-intent-cleanup-retain-v1".to_vec();
    fingerprint_field(&mut value, request.transfer_id.as_str().as_bytes());
    fingerprint_transfer_endpoint_fence(&mut value, &request.expected_source_fence);
    fingerprint_transfer_endpoint_fence(&mut value, &request.expected_target_fence);
    value.extend_from_slice(&request.expected_state_revision.get().to_be_bytes());
    value.push(u8::from(request.retain_remote_temporary_file_confirmed));
    value
}

fn matches_fences_and_revision(
    summary: &wire::SftpTransferIntentSummary,
    source_fence: &wire::SftpTransferEndpointFence,
    target_fence: &wire::SftpTransferEndpointFence,
    state_revision: WireSequence,
) -> bool {
    summary.source_fence == *source_fence
        && summary.target_fence == *target_fence
        && summary.state_revision == state_revision
}

fn resolve_cleaned_intent(
    summary: &mut wire::SftpTransferIntentSummary,
) -> Result<(), SftpRuntimeError> {
    if summary.commit_outcome == wire::SftpTransferCommitOutcome::Uncertain {
        return Err(SftpRuntimeError::Conflict);
    }
    summary.cleanup_residual = None;
    if summary.failure_code == Some(wire::SftpTransferFailureCode::CleanupIncomplete) {
        summary.failure_code = None;
        summary.state = match summary.commit_outcome {
            wire::SftpTransferCommitOutcome::Committed => wire::SftpTransferState::Completed,
            wire::SftpTransferCommitOutcome::NotCommitted => wire::SftpTransferState::Cancelled,
            wire::SftpTransferCommitOutcome::Uncertain => unreachable!("checked above"),
        };
    }
    summary.state_revision = WireSequence::new(summary.state_revision.get().saturating_add(1));
    Ok(())
}

impl SftpSessionService {
    async fn replay_intent_cleanup(
        &self,
        operation_id: &str,
        idempotency_key: &str,
        fingerprint: &[u8],
        kind: IntentCleanupReplayKind,
    ) -> Result<Option<wire::SftpTransferIntentSummary>, SftpProductionError> {
        let replays = self.intent_cleanup_replays.lock().await;
        let Some(replay) = replays.get(operation_id) else {
            return Ok(None);
        };
        if replay.kind != kind
            || replay.idempotency_key != idempotency_key
            || replay.fingerprint != fingerprint
        {
            return Err(SftpRuntimeError::Conflict.into());
        }
        Ok(Some(replay.result.clone()))
    }

    async fn record_intent_cleanup(
        &self,
        operation_id: String,
        idempotency_key: String,
        fingerprint: Vec<u8>,
        kind: IntentCleanupReplayKind,
        result: wire::SftpTransferIntentSummary,
    ) {
        let mut replays = self.intent_cleanup_replays.lock().await;
        if replays.len() >= MUTATION_LEDGER_CAPACITY
            && let Some(oldest) = replays.keys().next().cloned()
        {
            replays.remove(&oldest);
        }
        replays.insert(
            operation_id,
            IntentCleanupReplay {
                kind,
                idempotency_key,
                fingerprint,
                result,
            },
        );
    }

    pub(super) async fn retry_transfer_intent_cleanup(
        &self,
        request: wire::SftpTransferIntentCleanupRetryRequest,
    ) -> Result<wire::SftpTransferIntentSummary, SftpProductionError> {
        if request.idempotency_key.trim().is_empty() {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let operation_id = request.operation_id.to_string();
        let fingerprint = retry_fingerprint(&request);
        if let Some(result) = self
            .replay_intent_cleanup(
                &operation_id,
                &request.idempotency_key,
                &fingerprint,
                IntentCleanupReplayKind::Retry,
            )
            .await?
        {
            return Ok(result);
        }

        let transfer_key = request.transfer_id.to_string();
        let (target_session_key, temporary_target) = {
            let intents = self.transfer_intents.lock().await;
            let intent = intents
                .get(&transfer_key)
                .ok_or(SftpRuntimeError::InvalidInput)?;
            if !matches_fences_and_revision(
                &intent.summary,
                &request.expected_source_fence,
                &request.expected_target_fence,
                request.expected_state_revision,
            ) || intent.summary.state != wire::SftpTransferState::Failed
                || intent.summary.commit_outcome == wire::SftpTransferCommitOutcome::Uncertain
            {
                return Err(SftpRuntimeError::Conflict.into());
            }
            let wire::SftpTransferIntentCleanupResidual::RemoteTemporaryTarget {
                session_id,
                generation,
                path,
                ..
            } = intent
                .summary
                .cleanup_residual
                .as_ref()
                .ok_or(SftpRuntimeError::Conflict)?
            else {
                return Err(SftpRuntimeError::InvalidState.into());
            };
            let wire::SftpTransferEndpointFence::RemoteSession {
                session_id: target_session_id,
                generation: target_generation,
            } = &intent.summary.target_fence
            else {
                return Err(SftpRuntimeError::InvalidState.into());
            };
            if session_id != target_session_id || generation != target_generation {
                return Err(SftpRuntimeError::Conflict.into());
            }
            (
                session_id.to_string(),
                RemotePath::parse(path.bytes.clone())?,
            )
        };

        let host_id = {
            let records = self.records.lock().await;
            let record = records
                .get(&target_session_key)
                .ok_or(SftpRuntimeError::Conflict)?;
            HostId::parse(
                record
                    .actor
                    .summary()
                    .host_id
                    .as_deref()
                    .ok_or(SftpRuntimeError::InvalidState)?,
            )
            .map_err(|_| SftpRuntimeError::InvalidState)?
        };
        let factory = SftpTransportFactory::new(
            &self.hosts,
            &self.vault,
            &self.transient_credentials,
            &self.ssh_agent,
        );
        let profile =
            factory.resolve_saved_host(&host_id, request.expected_target_host_state_version)?;
        let verifier = Arc::new(TrustedSftpHostKeyVerifier {
            hosts: self.hosts.clone(),
        });
        let mut interaction = NonInteractiveSftpConnectionInteraction;
        let connection = factory.connect(profile, verifier, &mut interaction).await?;
        let mut cleanup_actor = SftpSessionActor::new(
            SftpSessionId::parse(uuid::Uuid::now_v7().to_string())?,
            host_id.to_string(),
        )?;
        let cleanup_generation = cleanup_actor.start("transfer-intent-remote-cleanup")?;
        let cleanup_session = connection
            .open_subsystem(&mut cleanup_actor, cleanup_generation)
            .await?;
        let cleanup = cleanup_session
            .cleanup_temporary_target(cleanup_generation, &temporary_target)
            .await;
        let _ = cleanup_session.disconnect().await;
        if !matches!(cleanup?, CleanupOutcome::Cleaned) {
            return Err(SftpRuntimeError::CleanupIncomplete.into());
        }

        let result = {
            let mut intents = self.transfer_intents.lock().await;
            let intent = intents
                .get_mut(&transfer_key)
                .ok_or(SftpRuntimeError::Conflict)?;
            if !matches_fences_and_revision(
                &intent.summary,
                &request.expected_source_fence,
                &request.expected_target_fence,
                request.expected_state_revision,
            ) || intent.summary.cleanup_residual.is_none()
            {
                return Err(SftpRuntimeError::Conflict.into());
            }
            resolve_cleaned_intent(&mut intent.summary)?;
            intent.summary.clone()
        };
        self.intent_cleanup_exit_authorizations
            .lock()
            .await
            .remove(&transfer_key);
        self.record_intent_cleanup(
            operation_id,
            request.idempotency_key,
            fingerprint,
            IntentCleanupReplayKind::Retry,
            result.clone(),
        )
        .await;
        Ok(result)
    }

    pub(super) async fn retain_transfer_intent_cleanup_for_exit(
        &self,
        request: wire::SftpTransferIntentCleanupRetainRequest,
    ) -> Result<wire::SftpTransferIntentSummary, SftpProductionError> {
        if request.idempotency_key.trim().is_empty()
            || !request.retain_remote_temporary_file_confirmed
        {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let operation_id = request.operation_id.to_string();
        let fingerprint = retain_fingerprint(&request);
        if let Some(result) = self
            .replay_intent_cleanup(
                &operation_id,
                &request.idempotency_key,
                &fingerprint,
                IntentCleanupReplayKind::RetainForExit,
            )
            .await?
        {
            return Ok(result);
        }
        let transfer_key = request.transfer_id.to_string();
        let result = {
            let intents = self.transfer_intents.lock().await;
            let intent = intents
                .get(&transfer_key)
                .ok_or(SftpRuntimeError::InvalidInput)?;
            if !matches_fences_and_revision(
                &intent.summary,
                &request.expected_source_fence,
                &request.expected_target_fence,
                request.expected_state_revision,
            ) || intent.summary.state != wire::SftpTransferState::Failed
                || !matches!(
                    intent.summary.cleanup_residual,
                    Some(wire::SftpTransferIntentCleanupResidual::RemoteTemporaryTarget { .. })
                )
            {
                return Err(SftpRuntimeError::Conflict.into());
            }
            intent.summary.clone()
        };
        self.intent_cleanup_exit_authorizations.lock().await.insert(
            transfer_key,
            IntentCleanupExitAuthorization {
                source_fence: result.source_fence.clone(),
                target_fence: result.target_fence.clone(),
                state_revision: result.state_revision,
            },
        );
        self.record_intent_cleanup(
            operation_id,
            request.idempotency_key,
            fingerprint,
            IntentCleanupReplayKind::RetainForExit,
            result.clone(),
        )
        .await;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(commit_outcome: wire::SftpTransferCommitOutcome) -> wire::SftpTransferIntentSummary {
        let session_id = wire::SftpSessionId::new();
        let fence = wire::SftpTransferEndpointFence::RemoteSession {
            session_id: session_id.clone(),
            generation: WireSequence::new(4),
        };
        wire::SftpTransferIntentSummary {
            transfer_id: wire::TransferId::new(),
            direction: wire::SftpTransferIntentDirection::ServerToServer,
            source_display_name: "source.bin".to_owned(),
            target_display_name: "source.bin".to_owned(),
            source_pane_id: "source-pane".to_owned(),
            target_pane_id: "target-pane".to_owned(),
            source_endpoint_revision: WireSequence::new(2),
            target_endpoint_revision: WireSequence::new(3),
            source_fence: fence.clone(),
            target_fence: fence,
            expected_bytes: 8,
            transferred_bytes: 8,
            bytes_per_second: None,
            remaining_seconds: None,
            state_revision: WireSequence::new(7),
            state: wire::SftpTransferState::Failed,
            commit_outcome,
            failure_code: Some(wire::SftpTransferFailureCode::CleanupIncomplete),
            cleanup_residual: Some(
                wire::SftpTransferIntentCleanupResidual::RemoteTemporaryTarget {
                    session_id,
                    generation: WireSequence::new(4),
                    path: wire::SftpRemotePath {
                        bytes: b"/.source.part".to_vec(),
                    },
                    display_path: "/.source.part".to_owned(),
                },
            ),
        }
    }

    #[test]
    fn confirmed_cleanup_converges_only_proven_commit_outcomes() {
        let mut committed = summary(wire::SftpTransferCommitOutcome::Committed);
        resolve_cleaned_intent(&mut committed).unwrap();
        assert_eq!(committed.state, wire::SftpTransferState::Completed);
        assert_eq!(committed.state_revision.get(), 8);
        assert!(committed.cleanup_residual.is_none());

        let mut not_committed = summary(wire::SftpTransferCommitOutcome::NotCommitted);
        resolve_cleaned_intent(&mut not_committed).unwrap();
        assert_eq!(not_committed.state, wire::SftpTransferState::Cancelled);

        let mut uncertain = summary(wire::SftpTransferCommitOutcome::Uncertain);
        assert_eq!(
            resolve_cleaned_intent(&mut uncertain),
            Err(SftpRuntimeError::Conflict)
        );
        assert!(uncertain.cleanup_residual.is_some());
    }

    #[test]
    fn retain_authorization_is_invalidated_by_any_revision_change() {
        let mut value = summary(wire::SftpTransferCommitOutcome::Uncertain);
        let authorization = IntentCleanupExitAuthorization {
            source_fence: value.source_fence.clone(),
            target_fence: value.target_fence.clone(),
            state_revision: value.state_revision,
        };
        assert!(authorization.matches(&value));
        value.state_revision = WireSequence::new(value.state_revision.get() + 1);
        assert!(!authorization.matches(&value));
    }
}
