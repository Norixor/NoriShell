use std::collections::{BTreeMap, VecDeque};

use norishell_core_api::{
    CoreApiError, CredentialRefId, RequestId, SshAttachAttemptId, SshAttachmentId, SshChannelId,
    SshHostKeyChallengeId, SshHostKeyDecision, SshHostKeyDecisionRequest,
    SshLoginAutomationTakeoverRequest, SshOpenAttemptId, SshSessionAttachRequest,
    SshSessionDetachRequest, SshSessionDisconnectRequest, SshSessionId,
    SshSessionInputLeaseAcquireRequest, SshSessionLastDetachConfirmation, SshSessionOpenRequest,
    SshSessionReconnectRequest, SshSessionTarget, SshTerminalInputFocusChangeRequest, SshViewId,
    TerminalInputFocusChangeRequest, TerminalInputFocusTarget, WireSequence,
};

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum SshOperationFingerprint {
    Open {
        open_attempt_id: SshOpenAttemptId,
        attach_attempt_id: SshAttachAttemptId,
        target: SshSessionTarget,
        credential_ref_id: Option<CredentialRefId>,
        view_id: SshViewId,
        rows: u16,
        cols: u16,
    },
    Attach {
        session_id: SshSessionId,
        expected_generation: WireSequence,
        expected_state_revision: WireSequence,
        attach_attempt_id: SshAttachAttemptId,
        view_id: SshViewId,
        after_output_seq: Option<WireSequence>,
    },
    Detach {
        session_id: SshSessionId,
        expected_generation: WireSequence,
        expected_state_revision: WireSequence,
        attachment_id: SshAttachmentId,
        view_id: SshViewId,
        intent: norishell_core_api::SshSessionDetachIntent,
        confirmation: Option<SshSessionLastDetachConfirmation>,
    },
    HostKeyDecision {
        session_id: SshSessionId,
        expected_generation: WireSequence,
        challenge_id: SshHostKeyChallengeId,
        expected_state_revision: WireSequence,
        attachment_id: SshAttachmentId,
        view_id: SshViewId,
        decision: SshHostKeyDecision,
    },
    LeaseAcquire {
        session_id: SshSessionId,
        expected_generation: WireSequence,
        expected_state_revision: WireSequence,
        attachment_id: SshAttachmentId,
        view_id: SshViewId,
    },
    FocusChange {
        expected_focus_epoch: WireSequence,
        target: Option<TerminalInputFocusTarget>,
    },
    Reconnect {
        session_id: SshSessionId,
        expected_generation: WireSequence,
        expected_state_revision: WireSequence,
        attachment_id: SshAttachmentId,
        view_id: SshViewId,
        credential_ref_id: Option<CredentialRefId>,
        rows: u16,
        cols: u16,
    },
    LoginAutomationTakeover {
        session_id: SshSessionId,
        expected_generation: WireSequence,
        expected_state_revision: WireSequence,
        expected_focus_epoch: WireSequence,
        channel_id: SshChannelId,
        attachment_id: SshAttachmentId,
        view_id: SshViewId,
    },
    Disconnect {
        session_id: SshSessionId,
        expected_generation: WireSequence,
        expected_state_revision: WireSequence,
    },
}

impl From<&SshSessionOpenRequest> for SshOperationFingerprint {
    fn from(request: &SshSessionOpenRequest) -> Self {
        Self::Open {
            open_attempt_id: request.open_attempt_id.clone(),
            attach_attempt_id: request.attach_attempt_id.clone(),
            target: request.target.clone(),
            credential_ref_id: request.credential_ref_id.clone(),
            view_id: request.view_id.clone(),
            rows: request.rows,
            cols: request.cols,
        }
    }
}

impl From<&SshSessionAttachRequest> for SshOperationFingerprint {
    fn from(request: &SshSessionAttachRequest) -> Self {
        Self::Attach {
            session_id: request.session_id.clone(),
            expected_generation: request.expected_generation,
            expected_state_revision: request.expected_state_revision,
            attach_attempt_id: request.attach_attempt_id.clone(),
            view_id: request.view_id.clone(),
            after_output_seq: request.after_output_seq,
        }
    }
}

impl From<&SshSessionDetachRequest> for SshOperationFingerprint {
    fn from(request: &SshSessionDetachRequest) -> Self {
        Self::Detach {
            session_id: request.session_id.clone(),
            expected_generation: request.expected_generation,
            expected_state_revision: request.expected_state_revision,
            attachment_id: request.attachment_id.clone(),
            view_id: request.view_id.clone(),
            intent: request.intent,
            confirmation: request.confirmation.clone(),
        }
    }
}

impl From<&SshHostKeyDecisionRequest> for SshOperationFingerprint {
    fn from(request: &SshHostKeyDecisionRequest) -> Self {
        Self::HostKeyDecision {
            session_id: request.session_id.clone(),
            expected_generation: request.expected_generation,
            challenge_id: request.challenge_id.clone(),
            expected_state_revision: request.expected_state_revision,
            attachment_id: request.attachment_id.clone(),
            view_id: request.view_id.clone(),
            decision: request.decision,
        }
    }
}

impl From<&SshSessionInputLeaseAcquireRequest> for SshOperationFingerprint {
    fn from(request: &SshSessionInputLeaseAcquireRequest) -> Self {
        Self::LeaseAcquire {
            session_id: request.session_id.clone(),
            expected_generation: request.expected_generation,
            expected_state_revision: request.expected_state_revision,
            attachment_id: request.attachment_id.clone(),
            view_id: request.view_id.clone(),
        }
    }
}

impl From<&SshTerminalInputFocusChangeRequest> for SshOperationFingerprint {
    fn from(request: &SshTerminalInputFocusChangeRequest) -> Self {
        Self::FocusChange {
            expected_focus_epoch: request.expected_focus_epoch,
            target: request.target.clone().map(TerminalInputFocusTarget::Ssh),
        }
    }
}

impl From<&TerminalInputFocusChangeRequest> for SshOperationFingerprint {
    fn from(request: &TerminalInputFocusChangeRequest) -> Self {
        Self::FocusChange {
            expected_focus_epoch: request.expected_focus_epoch,
            target: request.target.clone(),
        }
    }
}

impl From<&SshSessionReconnectRequest> for SshOperationFingerprint {
    fn from(request: &SshSessionReconnectRequest) -> Self {
        Self::Reconnect {
            session_id: request.session_id.clone(),
            expected_generation: request.expected_generation,
            expected_state_revision: request.expected_state_revision,
            attachment_id: request.attachment_id.clone(),
            view_id: request.view_id.clone(),
            credential_ref_id: request.credential_ref_id.clone(),
            rows: request.rows,
            cols: request.cols,
        }
    }
}

impl From<&SshLoginAutomationTakeoverRequest> for SshOperationFingerprint {
    fn from(request: &SshLoginAutomationTakeoverRequest) -> Self {
        Self::LoginAutomationTakeover {
            session_id: request.session_id.clone(),
            expected_generation: request.expected_generation,
            expected_state_revision: request.expected_state_revision,
            expected_focus_epoch: request.expected_focus_epoch,
            channel_id: request.channel_id.clone(),
            attachment_id: request.attachment_id.clone(),
            view_id: request.view_id.clone(),
        }
    }
}

impl From<&SshSessionDisconnectRequest> for SshOperationFingerprint {
    fn from(request: &SshSessionDisconnectRequest) -> Self {
        Self::Disconnect {
            session_id: request.session_id.clone(),
            expected_generation: request.expected_generation,
            expected_state_revision: request.expected_state_revision,
        }
    }
}

#[derive(Clone)]
pub(crate) enum StoredSshOperationResult<Outcome> {
    Ok(Outcome),
    Err(CoreApiError),
}

impl<Outcome> StoredSshOperationResult<Outcome> {
    pub(crate) fn from_actor_result(result: Result<Outcome, Box<CoreApiError>>) -> Self {
        match result {
            Ok(value) => Self::Ok(value),
            Err(error) => Self::Err(*error),
        }
    }
}

impl<Outcome: Clone> StoredSshOperationResult<Outcome> {
    pub(crate) fn replay(&self, request_id: RequestId) -> Result<Outcome, Box<CoreApiError>> {
        match self {
            Self::Ok(value) => Ok(value.clone()),
            Self::Err(error) => {
                let mut error = error.clone();
                error.request_id = Some(request_id);
                Err(Box::new(error))
            }
        }
    }
}

/// Bounded actor-local cache for one class of SSH control operations.
///
/// Callers provide a secret-free request fingerprint. An exact operation
/// replay returns the original outcome, while reusing an operation id with a
/// different idempotency key or fingerprint fails closed. The actor remains
/// the serialization boundary, so this registry never needs cross-thread
/// mutation or an in-progress waiter list.
pub(crate) struct SshOperationLedger<Fingerprint, Outcome> {
    capacity: usize,
    entries: BTreeMap<String, OperationEntry<Fingerprint, Outcome>>,
    insertion_order: VecDeque<String>,
}

struct OperationEntry<Fingerprint, Outcome> {
    idempotency_key: String,
    fingerprint: Fingerprint,
    outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SshOperationLookup<Outcome> {
    Missing,
    Replay(Outcome),
    Conflict,
}

impl<Fingerprint, Outcome> SshOperationLedger<Fingerprint, Outcome>
where
    Fingerprint: PartialEq,
    Outcome: Clone,
{
    pub(crate) fn new(capacity: usize) -> Self {
        assert!(
            capacity > 0,
            "SSH operation ledger capacity must be positive"
        );
        Self {
            capacity,
            entries: BTreeMap::new(),
            insertion_order: VecDeque::new(),
        }
    }

    pub(crate) fn lookup(
        &self,
        operation_id: &str,
        idempotency_key: &str,
        fingerprint: &Fingerprint,
    ) -> SshOperationLookup<Outcome> {
        let Some(entry) = self.entries.get(operation_id) else {
            return SshOperationLookup::Missing;
        };
        if entry.idempotency_key == idempotency_key && entry.fingerprint == *fingerprint {
            SshOperationLookup::Replay(entry.outcome.clone())
        } else {
            SshOperationLookup::Conflict
        }
    }

    /// Records the first terminal outcome for an operation id. If the same
    /// operation races through a future call site, the original outcome wins.
    pub(crate) fn record(
        &mut self,
        operation_id: String,
        idempotency_key: String,
        fingerprint: Fingerprint,
        outcome: Outcome,
    ) -> SshOperationLookup<Outcome> {
        match self.lookup(&operation_id, &idempotency_key, &fingerprint) {
            SshOperationLookup::Missing => {}
            existing => return existing,
        }

        while self.entries.len() >= self.capacity {
            let Some(expired_operation_id) = self.insertion_order.pop_front() else {
                break;
            };
            self.entries.remove(&expired_operation_id);
        }
        self.insertion_order.push_back(operation_id.clone());
        self.entries.insert(
            operation_id,
            OperationEntry {
                idempotency_key,
                fingerprint,
                outcome: outcome.clone(),
            },
        );
        SshOperationLookup::Replay(outcome)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use norishell_core_api::{CoreApiError, ErrorCategory, RequestId, RetryStrategy};

    use super::{SshOperationLedger, SshOperationLookup, StoredSshOperationResult};

    #[test]
    fn exact_replay_returns_the_original_outcome() {
        let mut ledger = SshOperationLedger::new(4);
        assert_eq!(
            ledger.lookup("operation-1", "same-key", &("session-1", 2_u64)),
            SshOperationLookup::Missing,
        );
        assert_eq!(
            ledger.record(
                "operation-1".to_owned(),
                "same-key".to_owned(),
                ("session-1", 2_u64),
                "accepted",
            ),
            SshOperationLookup::Replay("accepted"),
        );
        assert_eq!(
            ledger.lookup("operation-1", "same-key", &("session-1", 2_u64)),
            SshOperationLookup::Replay("accepted"),
        );
    }

    #[test]
    fn reused_operation_id_with_changed_input_conflicts() {
        let mut ledger = SshOperationLedger::new(4);
        ledger.record(
            "operation-1".to_owned(),
            "same-key".to_owned(),
            ("session-1", 2_u64),
            "accepted",
        );

        assert_eq!(
            ledger.lookup("operation-1", "different-key", &("session-1", 2_u64)),
            SshOperationLookup::Conflict,
        );
        assert_eq!(
            ledger.lookup("operation-1", "same-key", &("session-1", 3_u64)),
            SshOperationLookup::Conflict,
        );
    }

    #[test]
    fn capacity_evicts_the_oldest_completed_operation() {
        let mut ledger = SshOperationLedger::new(2);
        for operation in ["operation-1", "operation-2", "operation-3"] {
            ledger.record(operation.to_owned(), "key".to_owned(), operation, operation);
        }

        assert_eq!(
            ledger.lookup("operation-1", "key", &"operation-1"),
            SshOperationLookup::Missing,
        );
        assert_eq!(
            ledger.lookup("operation-2", "key", &"operation-2"),
            SshOperationLookup::Replay("operation-2"),
        );
        assert_eq!(
            ledger.lookup("operation-3", "key", &"operation-3"),
            SshOperationLookup::Replay("operation-3"),
        );
    }

    #[test]
    fn replayed_errors_use_the_current_request_identity() {
        let original_request_id = RequestId::new();
        let retry_request_id = RequestId::new();
        let stored =
            StoredSshOperationResult::<()>::from_actor_result(Err(Box::new(CoreApiError {
                code: "ssh_terminal.stale_fence".to_owned(),
                category: ErrorCategory::Conflict,
                retry_strategy: RetryStrategy::RefreshSnapshot,
                message_key: "errors.sshSession.staleFence".to_owned(),
                params: BTreeMap::new(),
                request_id: Some(original_request_id),
                diagnostic_id: None,
                conflict: None,
            })));

        let replayed = stored
            .replay(retry_request_id.clone())
            .expect_err("stored error");
        assert_eq!(replayed.request_id, Some(retry_request_id));
    }
}
