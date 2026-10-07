//! Activity foreground signal for resource notifications.
//!
//! tao reports `is_focused() == false` for every window on Android, so the
//! "background only" notification rule uses this flag instead. Kotlin
//! (`NotificationsBridge`) tracks resumed Activities of the app process —
//! MainActivity and Kotlin-hosted secure/tool surfaces alike — and stores the
//! result through `nativeNotificationEvent`.

use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) static ACTIVITY_RESUMED: AtomicBool = AtomicBool::new(false);

/// True while at least one Activity of the app process is resumed.
pub fn activity_resumed() -> bool {
    ACTIVITY_RESUMED.load(Ordering::Acquire)
}
