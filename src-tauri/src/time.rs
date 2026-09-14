use std::time::{SystemTime, UNIX_EPOCH};

/// Return a Unix millisecond timestamp that can be safely projected through Core API.
pub(crate) fn unix_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}
