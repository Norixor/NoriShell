use std::collections::BTreeMap;

use norishell_core_api::{CoreApiError, ErrorCategory, RequestId, RetryStrategy};

/// Build a public Core API error projection without internal details.
///
/// Each service still selects its stable code, message key, and retry policy; this helper only centralizes
/// the safe field defaults that must remain consistent across services.
pub(crate) fn core_error(
    request_id: RequestId,
    code: &str,
    category: ErrorCategory,
    retry_strategy: RetryStrategy,
    message_key: &str,
) -> Box<CoreApiError> {
    Box::new(CoreApiError {
        code: code.to_owned(),
        category,
        retry_strategy,
        message_key: message_key.to_owned(),
        params: BTreeMap::new(),
        request_id: Some(request_id),
        diagnostic_id: None,
        conflict: None,
    })
}
