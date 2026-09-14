use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{RequestId, WireSequence};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCategory {
    Validation,
    Conflict,
    Permission,
    Unavailable,
    Timeout,
    Internal,
    Incompatible,
    NeedsReconciliation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum RetryStrategy {
    Never,
    RefreshSnapshot,
    QueryOperation,
    AfterMilliseconds(#[ts(type = "number")] u64),
    WaitForUser,
    Upgrade,
    Reconcile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SafeConflictVersion {
    pub entity_version: Option<WireSequence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CoreApiError {
    pub code: String,
    pub category: ErrorCategory,
    pub retry_strategy: RetryStrategy,
    pub message_key: String,
    pub params: BTreeMap<String, String>,
    pub request_id: Option<RequestId>,
    pub diagnostic_id: Option<String>,
    pub conflict: Option<SafeConflictVersion>,
}

impl CoreApiError {
    #[must_use]
    pub fn incompatible(request_id: RequestId, supported_major: u16) -> Self {
        Self {
            code: "core_api.incompatible_major".to_owned(),
            category: ErrorCategory::Incompatible,
            retry_strategy: RetryStrategy::Upgrade,
            message_key: "errors.coreApi.incompatibleMajor".to_owned(),
            params: BTreeMap::from([("supportedMajor".to_owned(), supported_major.to_string())]),
            request_id: Some(request_id),
            diagnostic_id: None,
            conflict: None,
        }
    }

    #[must_use]
    pub fn safe_internal(request_id: RequestId, diagnostic_id: String) -> Self {
        Self {
            code: "core.internal".to_owned(),
            category: ErrorCategory::Internal,
            retry_strategy: RetryStrategy::Never,
            message_key: "errors.core.internal".to_owned(),
            params: BTreeMap::new(),
            request_id: Some(request_id),
            diagnostic_id: Some(diagnostic_id),
            conflict: None,
        }
    }

    #[must_use]
    pub fn missing_event_support(request_id: RequestId, event_kind: &str) -> Self {
        Self {
            code: "core_api.missing_required_event".to_owned(),
            category: ErrorCategory::Incompatible,
            retry_strategy: RetryStrategy::Upgrade,
            message_key: "errors.coreApi.missingRequiredEvent".to_owned(),
            params: BTreeMap::from([("eventKind".to_owned(), event_kind.to_owned())]),
            request_id: Some(request_id),
            diagnostic_id: None,
            conflict: None,
        }
    }
}
