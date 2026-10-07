use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use norishell_core_api::{
    CoreApiError, CredentialKind, CredentialRefId, ErrorCategory, RequestId, RetryStrategy,
    TransientCredentialPrepareRequest, TransientCredentialRef,
};
use norishell_ssh_transport::{Authentication, PrivateKeyInspectionError, inspect_private_key};
use subtle::ConstantTimeEq;
use tauri::State;
use zeroize::Zeroizing;

use crate::{core_api_error::core_error, time::unix_time_ms};

type CoreResult<T> = Result<T, Box<CoreApiError>>;

const TRANSIENT_CREDENTIAL_TTL: Duration = Duration::from_secs(180);
const MAX_PASSWORD_BYTES: usize = 64 * 1024;

struct TransientCredential {
    kind: CredentialKind,
    secret: Zeroizing<Vec<u8>>,
    passphrase: Option<Zeroizing<Vec<u8>>>,
    expires_at: Instant,
    expires_at_unix_ms: i64,
}

struct PreparedOperation {
    idempotency_key: String,
    credential_ref_id: CredentialRefId,
}

#[derive(Default)]
struct TransientCredentialState {
    credentials: BTreeMap<String, TransientCredential>,
    operations: BTreeMap<String, PreparedOperation>,
}

#[derive(Clone, Default)]
pub struct TransientCredentialService {
    state: Arc<Mutex<TransientCredentialState>>,
}

impl TransientCredentialService {
    pub fn prepare(
        &self,
        request: TransientCredentialPrepareRequest,
    ) -> CoreResult<TransientCredentialRef> {
        let request_id = request.meta.request_id;
        if request.idempotency_key.trim().is_empty() {
            return Err(validation_error(request_id));
        }

        let secret = Zeroizing::new(request.secret.into_bytes());
        let passphrase = request
            .passphrase
            .map(String::into_bytes)
            .map(Zeroizing::new);
        validate_material(
            request.kind,
            &secret,
            passphrase.as_deref().map(Vec::as_slice),
        )
        .map_err(|error| material_error(request_id.clone(), error))?;

        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cleanup_expired(&mut state);

        if let Some(operation) = state.operations.get(request.operation_id.as_str()) {
            if operation.idempotency_key != request.idempotency_key {
                return Err(conflict_error(request_id));
            }
            let existing = state
                .credentials
                .get(operation.credential_ref_id.as_str())
                .ok_or_else(|| conflict_error(request_id.clone()))?;
            if existing.kind != request.kind
                || !constant_time_equal(&existing.secret, &secret)
                || !constant_time_optional_equal(
                    existing.passphrase.as_deref().map(Vec::as_slice),
                    passphrase.as_deref().map(Vec::as_slice),
                )
            {
                return Err(conflict_error(request_id));
            }
            return Ok(TransientCredentialRef {
                credential_ref_id: operation.credential_ref_id.clone(),
                expires_at_unix_ms: existing.expires_at_unix_ms,
            });
        }

        let credential_ref_id = CredentialRefId::new();
        let expires_at_unix_ms = unix_time_ms().saturating_add(
            i64::try_from(TRANSIENT_CREDENTIAL_TTL.as_millis()).unwrap_or(i64::MAX),
        );
        state.credentials.insert(
            credential_ref_id.as_str().to_owned(),
            TransientCredential {
                kind: request.kind,
                secret,
                passphrase,
                expires_at: Instant::now() + TRANSIENT_CREDENTIAL_TTL,
                expires_at_unix_ms,
            },
        );
        state.operations.insert(
            request.operation_id.as_str().to_owned(),
            PreparedOperation {
                idempotency_key: request.idempotency_key,
                credential_ref_id: credential_ref_id.clone(),
            },
        );
        Ok(TransientCredentialRef {
            credential_ref_id,
            expires_at_unix_ms,
        })
    }

    pub fn contains(&self, credential_ref_id: &CredentialRefId) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cleanup_expired(&mut state);
        state.credentials.contains_key(credential_ref_id.as_str())
    }

    pub fn take(&self, credential_ref_id: &CredentialRefId) -> Option<Authentication> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cleanup_expired(&mut state);
        let credential = state.credentials.remove(credential_ref_id.as_str())?;
        state
            .operations
            .retain(|_, operation| operation.credential_ref_id != *credential_ref_id);
        Some(match credential.kind {
            CredentialKind::Password => Authentication::password(credential.secret.to_vec()),
            CredentialKind::PrivateKey => Authentication::private_key(
                credential.secret.to_vec(),
                credential.passphrase.map(|value| value.to_vec()),
            ),
        })
    }

    pub fn discard(&self, credential_ref_id: &CredentialRefId) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.credentials.remove(credential_ref_id.as_str());
        state
            .operations
            .retain(|_, operation| operation.credential_ref_id != *credential_ref_id);
    }
}

fn validate_material(
    kind: CredentialKind,
    secret: &[u8],
    passphrase: Option<&[u8]>,
) -> Result<(), PrivateKeyInspectionError> {
    match kind {
        CredentialKind::Password => {
            if secret.is_empty() || secret.len() > MAX_PASSWORD_BYTES || passphrase.is_some() {
                return Err(PrivateKeyInspectionError::InvalidFormat);
            }
        }
        CredentialKind::PrivateKey => {
            inspect_private_key(secret, passphrase)?;
        }
    }
    Ok(())
}

fn material_error(request_id: RequestId, error: PrivateKeyInspectionError) -> Box<CoreApiError> {
    match error {
        PrivateKeyInspectionError::InvalidFormat => validation_error(request_id),
        PrivateKeyInspectionError::PassphraseRequired => core_error(
            request_id,
            "credential_transient.passphrase_required",
            ErrorCategory::Validation,
            RetryStrategy::WaitForUser,
            "errors.credential.passphraseRequired",
        ),
        PrivateKeyInspectionError::IncorrectPassphrase => core_error(
            request_id,
            "credential_transient.incorrect_passphrase",
            ErrorCategory::Validation,
            RetryStrategy::WaitForUser,
            "errors.credential.incorrectPassphrase",
        ),
    }
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && bool::from(left.ct_eq(right))
}

fn constant_time_optional_equal(left: Option<&[u8]>, right: Option<&[u8]>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => constant_time_equal(left, right),
        _ => false,
    }
}

fn cleanup_expired(state: &mut TransientCredentialState) {
    let now = Instant::now();
    state
        .credentials
        .retain(|_, credential| credential.expires_at > now);
    state.operations.retain(|_, operation| {
        state
            .credentials
            .contains_key(operation.credential_ref_id.as_str())
    });
}

fn validation_error(request_id: RequestId) -> Box<CoreApiError> {
    core_error(
        request_id,
        "credential_transient.invalid_request",
        ErrorCategory::Validation,
        RetryStrategy::Never,
        "errors.transientCredential.invalidRequest",
    )
}

fn conflict_error(request_id: RequestId) -> Box<CoreApiError> {
    core_error(
        request_id,
        "credential_transient.idempotency_conflict",
        ErrorCategory::Conflict,
        RetryStrategy::Never,
        "errors.transientCredential.idempotencyConflict",
    )
}

#[tauri::command]
pub async fn credential_transient_prepare(
    request: TransientCredentialPrepareRequest,
    service: State<'_, TransientCredentialService>,
) -> CoreResult<TransientCredentialRef> {
    service.prepare(request)
}

#[cfg(test)]
mod tests {
    use norishell_core_api::{
        CredentialKind, OperationId, RequestId, RequestMeta, TransientCredentialPrepareRequest,
    };

    use super::TransientCredentialService;

    /// Encrypted keys without a passphrase, with a wrong one, and corrupted
    /// material each produce their own stable code.
    #[test]
    fn private_key_material_failures_have_distinct_codes() {
        use russh::keys::ssh_key::{LineEnding, PrivateKey};
        let key = PrivateKey::random(&mut rand::rng(), russh::keys::ssh_key::Algorithm::Ed25519)
            .expect("client key");
        let encrypted = key
            .encrypt(&mut rand::rng(), b"right-passphrase")
            .expect("encrypt")
            .to_openssh(LineEnding::LF)
            .expect("encode")
            .to_string();
        let service = TransientCredentialService::default();
        for (secret, passphrase, code) in [
            (
                encrypted.clone(),
                None,
                "credential_transient.passphrase_required",
            ),
            (
                encrypted.clone(),
                Some("wrong-passphrase"),
                "credential_transient.incorrect_passphrase",
            ),
            (
                "-----BEGIN OPENSSH PRIVATE KEY-----\nnot-a-key\n-----END OPENSSH PRIVATE KEY-----\n"
                    .to_owned(),
                None,
                "credential_transient.invalid_request",
            ),
        ] {
            let error = service
                .prepare(TransientCredentialPrepareRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                    operation_id: OperationId::new(),
                    idempotency_key: format!("material-{code}"),
                    kind: CredentialKind::PrivateKey,
                    secret,
                    passphrase: passphrase.map(str::to_owned),
                })
                .expect_err("invalid material is rejected");
            assert_eq!(error.code, code);
        }
        assert!(
            service
                .prepare(TransientCredentialPrepareRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                    operation_id: OperationId::new(),
                    idempotency_key: "material-ok".to_owned(),
                    kind: CredentialKind::PrivateKey,
                    secret: encrypted,
                    passphrase: Some("right-passphrase".to_owned()),
                })
                .is_ok()
        );
    }

    fn password_request(
        operation_id: OperationId,
        idempotency_key: &str,
        secret: &str,
    ) -> TransientCredentialPrepareRequest {
        TransientCredentialPrepareRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id,
            idempotency_key: idempotency_key.to_owned(),
            kind: CredentialKind::Password,
            secret: secret.to_owned(),
            passphrase: None,
        }
    }

    #[test]
    fn credential_is_one_shot() {
        let service = TransientCredentialService::default();
        let reference = service
            .prepare(password_request(OperationId::new(), "one-shot", "secret"))
            .expect("prepare transient credential");

        assert!(service.contains(&reference.credential_ref_id));
        assert!(service.take(&reference.credential_ref_id).is_some());
        assert!(!service.contains(&reference.credential_ref_id));
        assert!(service.take(&reference.credential_ref_id).is_none());
    }

    #[test]
    fn exact_retry_reuses_reference_but_changed_secret_conflicts() {
        let service = TransientCredentialService::default();
        let operation_id = OperationId::new();
        let first = service
            .prepare(password_request(operation_id.clone(), "retry", "secret"))
            .expect("prepare transient credential");
        let replay = service
            .prepare(password_request(operation_id.clone(), "retry", "secret"))
            .expect("replay transient credential");

        assert_eq!(first, replay);
        assert!(
            service
                .prepare(password_request(operation_id, "retry", "different"))
                .is_err()
        );
    }

    #[test]
    fn discard_removes_material() {
        let service = TransientCredentialService::default();
        let reference = service
            .prepare(password_request(OperationId::new(), "discard", "secret"))
            .expect("prepare transient credential");

        service.discard(&reference.credential_ref_id);
        assert!(!service.contains(&reference.credential_ref_id));
    }
}
