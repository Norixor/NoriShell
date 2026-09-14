use std::{
    collections::{BTreeMap, VecDeque},
    future::Future,
    sync::Arc,
};

use norishell_core_api::{
    CoreApiError, ErrorCategory, RequestId, RetryStrategy, TerminalInputFocusChangeRequest,
    TerminalInputFocusChangeResponse, TerminalInputFocusTarget, WireSequence,
};
use tokio::sync::Mutex;

const FOCUS_OPERATION_LEDGER_CAPACITY: usize = 1_024;
type DesktopInvalidator = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, PartialEq, Eq)]
struct FocusFingerprint {
    idempotency_key: String,
    expected_focus_epoch: WireSequence,
    target: Option<TerminalInputFocusTarget>,
}

impl From<&TerminalInputFocusChangeRequest> for FocusFingerprint {
    fn from(request: &TerminalInputFocusChangeRequest) -> Self {
        Self {
            idempotency_key: request.idempotency_key.clone(),
            expected_focus_epoch: request.expected_focus_epoch,
            target: request.target.clone(),
        }
    }
}

#[derive(Clone)]
struct StoredFocusOperation {
    fingerprint: FocusFingerprint,
    result: Result<TerminalInputFocusChangeResponse, CoreApiError>,
}

#[derive(Default)]
struct BrokerState {
    focus_operations: BTreeMap<String, StoredFocusOperation>,
    focus_operation_order: VecDeque<String>,
}

/// A single ordering domain for process-wide operations sensitive to terminal focus.
///
/// Resource actors still own their SSH channels, local PTYs, and Telnet sockets. Every cross-actor operation
/// that changes or consumes the global focus fence must complete in this ordering domain, closing the race
/// between taking a snapshot and performing the actual write.
#[derive(Clone, Default)]
pub(crate) struct TerminalFocusBroker {
    state: Arc<Mutex<BrokerState>>,
    desktop_invalidator: Arc<std::sync::RwLock<Option<DesktopInvalidator>>>,
}

impl TerminalFocusBroker {
    pub(crate) fn register_desktop_invalidator(&self, invalidate: DesktopInvalidator) {
        *self
            .desktop_invalidator
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(invalidate);
    }

    pub(crate) async fn linearize<T>(&self, operation: impl Future<Output = T>) -> T {
        let _guard = self.state.lock().await;
        operation.await
    }

    pub(crate) async fn linearize_focus_change(
        &self,
        request: &TerminalInputFocusChangeRequest,
        operation: impl Future<Output = Result<TerminalInputFocusChangeResponse, Box<CoreApiError>>>,
    ) -> Result<TerminalInputFocusChangeResponse, Box<CoreApiError>> {
        let mut state = self.state.lock().await;
        let operation_id = request.operation_id.as_str().to_owned();
        let fingerprint = FocusFingerprint::from(request);
        if let Some(stored) = state.focus_operations.get(&operation_id) {
            if stored.fingerprint != fingerprint {
                return Err(focus_conflict(request.meta.request_id.clone()));
            }
            return replay_result(&stored.result, request.meta.request_id.clone());
        }

        // Revoke desktop input within the same ordering domain; do not wait for route unmount to release remote keys.
        if request.target.is_some()
            && let Some(invalidate) = self
                .desktop_invalidator
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
        {
            invalidate();
        }
        let result = operation.await;
        let stored_result = result.clone().map_err(|error| *error);
        state.focus_operations.insert(
            operation_id.clone(),
            StoredFocusOperation {
                fingerprint,
                result: stored_result,
            },
        );
        state.focus_operation_order.push_back(operation_id);
        while state.focus_operation_order.len() > FOCUS_OPERATION_LEDGER_CAPACITY {
            if let Some(expired) = state.focus_operation_order.pop_front() {
                state.focus_operations.remove(&expired);
            }
        }
        result
    }
}

fn replay_result(
    stored: &Result<TerminalInputFocusChangeResponse, CoreApiError>,
    request_id: RequestId,
) -> Result<TerminalInputFocusChangeResponse, Box<CoreApiError>> {
    stored.clone().map_err(|mut error| {
        error.request_id = Some(request_id);
        Box::new(error)
    })
}

fn focus_conflict(request_id: RequestId) -> Box<CoreApiError> {
    crate::core_api_error::core_error(
        request_id,
        "ssh_terminal.stale_fence",
        ErrorCategory::Conflict,
        RetryStrategy::RefreshSnapshot,
        "errors.sshSession.staleFence",
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use norishell_core_api::{OperationId, RequestMeta};
    use tokio::sync::oneshot;

    use super::*;

    #[tokio::test]
    async fn terminal_focus_revokes_desktop_before_grant_and_does_not_repeat_on_replay() {
        use norishell_core_api::*;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let broker = TerminalFocusBroker::default();
        let revoked = Arc::new(AtomicUsize::new(0));
        let counter = revoked.clone();
        broker.register_desktop_invalidator(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        let mut request = focus_request(OperationId::new(), "desktop-to-local");
        request.target = Some(TerminalInputFocusTarget::Local(
            LocalTerminalInputFocusTarget {
                session_id: LocalSessionId::new(),
                expected_generation: WireSequence::new(1),
                expected_state_revision: WireSequence::new(1),
                pty_id: LocalPtyId::new(),
                attachment_id: LocalAttachmentId::new(),
                view_id: LocalViewId::new(),
            },
        ));
        let response = TerminalInputFocusChangeResponse {
            focus_epoch: WireSequence::new(1),
            target: request.target.clone(),
            lease: None,
        };
        broker
            .linearize_focus_change(&request, async {
                assert_eq!(revoked.load(Ordering::SeqCst), 1);
                Ok(response)
            })
            .await
            .unwrap();
        broker
            .linearize_focus_change(&request, async { panic!("replayed focus grant") })
            .await
            .unwrap();
        assert_eq!(revoked.load(Ordering::SeqCst), 1);
    }

    fn focus_request(
        operation_id: OperationId,
        idempotency_key: &str,
    ) -> TerminalInputFocusChangeRequest {
        TerminalInputFocusChangeRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id,
            idempotency_key: idempotency_key.to_owned(),
            expected_focus_epoch: WireSequence::new(0),
            target: None,
        }
    }

    #[tokio::test]
    async fn linearization_blocks_a_second_focus_sensitive_operation() {
        let broker = TerminalFocusBroker::default();
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let first_broker = broker.clone();
        let first = tokio::spawn(async move {
            first_broker
                .linearize(async move {
                    let _ = entered_tx.send(());
                    let _ = release_rx.await;
                })
                .await;
        });
        entered_rx.await.expect("first operation entered");

        let second_broker = broker.clone();
        let (second_tx, second_rx) = oneshot::channel();
        let second = tokio::spawn(async move {
            second_broker
                .linearize(async move {
                    let _ = second_tx.send(());
                })
                .await;
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(25), second_rx)
                .await
                .is_err()
        );
        let _ = release_tx.send(());
        first.await.expect("first operation");
        second.await.expect("second operation");
    }

    #[tokio::test]
    async fn exact_focus_replay_keeps_the_original_response_and_conflicts_on_payload_change() {
        let broker = TerminalFocusBroker::default();
        let operation_id = OperationId::new();
        let request = focus_request(operation_id.clone(), "same");
        let response = TerminalInputFocusChangeResponse {
            focus_epoch: WireSequence::new(1),
            target: None,
            lease: None,
        };
        let first = broker
            .linearize_focus_change(&request, async { Ok(response.clone()) })
            .await
            .expect("first result");
        let replay = broker
            .linearize_focus_change(&request, async {
                panic!("exact replay must not execute twice")
            })
            .await
            .expect("replay");
        assert_eq!(replay, first);

        let conflict = focus_request(operation_id, "different");
        let error = broker
            .linearize_focus_change(&conflict, async {
                panic!("conflicting replay must not execute")
            })
            .await
            .expect_err("conflict");
        assert_eq!(error.code, "ssh_terminal.stale_fence");
    }
}
