use std::time::Duration;

use norishell_core_api::{PluginApiErrorCode, PluginApiResourceEventKind};
use tokio::sync::watch;

use super::{ResourceEventWriter, ResourceFence, ResourceOwner, ResourceRegistry};

pub(crate) const MIN_TIMER_MILLISECONDS: u32 = 100;
pub(crate) const MAX_TIMER_MILLISECONDS: u32 = 24 * 60 * 60 * 1000;

pub(crate) fn start_timer(
    resources: &ResourceRegistry,
    owner: ResourceOwner,
    delay_ms: u32,
    interval_ms: Option<u32>,
    fence: ResourceFence,
) -> Result<String, PluginApiErrorCode> {
    validate_timer(delay_ms, interval_ms)?;
    if !fence() {
        return Err(PluginApiErrorCode::Revoked);
    }
    resources.spawn(owner, "timer", move |cancel, events| async move {
        drive_timer(cancel, events, delay_ms, interval_ms, fence).await
    })
}

pub(crate) fn validate_timer(
    delay_ms: u32,
    interval_ms: Option<u32>,
) -> Result<(), PluginApiErrorCode> {
    if !(MIN_TIMER_MILLISECONDS..=MAX_TIMER_MILLISECONDS).contains(&delay_ms)
        || interval_ms.is_some_and(|interval| {
            !(MIN_TIMER_MILLISECONDS..=MAX_TIMER_MILLISECONDS).contains(&interval)
        })
    {
        return Err(PluginApiErrorCode::InvalidRequest);
    }
    Ok(())
}

async fn drive_timer(
    mut cancel: watch::Receiver<bool>,
    events: ResourceEventWriter,
    delay_ms: u32,
    interval_ms: Option<u32>,
    fence: ResourceFence,
) -> Result<(), PluginApiErrorCode> {
    match wait_for_timer(&mut cancel, &fence, delay_ms).await {
        TimerWait::Cancelled => return Ok(()),
        TimerWait::Revoked => {
            let _ = events.emit(PluginApiResourceEventKind::Cancelled {});
            return Ok(());
        }
        TimerWait::Elapsed => {}
    }
    loop {
        let _ = events.emit(PluginApiResourceEventKind::TimerFired {});
        let Some(interval_ms) = interval_ms else {
            return Ok(());
        };
        match wait_for_timer(&mut cancel, &fence, interval_ms).await {
            TimerWait::Cancelled => return Ok(()),
            TimerWait::Revoked => {
                let _ = events.emit(PluginApiResourceEventKind::Cancelled {});
                return Ok(());
            }
            TimerWait::Elapsed => {}
        }
    }
}

enum TimerWait {
    Elapsed,
    Cancelled,
    Revoked,
}

async fn wait_for_timer(
    cancel: &mut watch::Receiver<bool>,
    fence: &ResourceFence,
    milliseconds: u32,
) -> TimerWait {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(u64::from(milliseconds));
    loop {
        if !fence() {
            return TimerWait::Revoked;
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return TimerWait::Elapsed;
        }
        tokio::select! {
            _ = tokio::time::sleep(remaining.min(Duration::from_millis(u64::from(MIN_TIMER_MILLISECONDS)))) => {}
            _changed = cancel.changed() => {
                return TimerWait::Cancelled;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use norishell_core_api::{PluginId, WireSequence};
    use std::sync::Arc;

    fn owner() -> ResourceOwner {
        ResourceOwner {
            plugin_id: PluginId::parse("org.norishell.timer-test").unwrap(),
            signer: "a".repeat(64),
            package: "b".repeat(64),
            generation: WireSequence::new(1),
        }
    }

    #[tokio::test]
    async fn one_shot_timer_emits_a_real_event_then_cleans_up() {
        let resources = ResourceRegistry::default();
        let owner = owner();
        let handle = start_timer(
            &resources,
            owner.clone(),
            MIN_TIMER_MILLISECONDS,
            None,
            Arc::new(|| true),
        )
        .unwrap();
        tokio::time::sleep(Duration::from_millis(u64::from(
            MIN_TIMER_MILLISECONDS + 30,
        )))
        .await;
        let (events, backpressured) = resources.take_events(&owner, &handle, 1).unwrap();
        assert!(!backpressured);
        assert!(
            matches!(events.as_slice(), [event] if matches!(event.kind, PluginApiResourceEventKind::TimerFired {}))
        );
        resources.close(&owner, &handle).await.unwrap();
        assert!(resources.list(&owner).is_empty());
    }

    #[test]
    fn timer_bounds_reject_zero_and_out_of_range_intervals() {
        assert!(validate_timer(0, None).is_err());
        assert!(validate_timer(MIN_TIMER_MILLISECONDS, Some(0)).is_err());
        assert!(validate_timer(MAX_TIMER_MILLISECONDS + 1, None).is_err());
    }

    #[tokio::test]
    async fn revoked_fence_prevents_creation_and_stops_existing_timer() {
        let resources = ResourceRegistry::default();
        let owner = owner();
        assert_eq!(
            start_timer(
                &resources,
                owner.clone(),
                MIN_TIMER_MILLISECONDS,
                None,
                Arc::new(|| false)
            ),
            Err(PluginApiErrorCode::Revoked)
        );
        let allowed = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let fence = {
            let allowed = allowed.clone();
            Arc::new(move || allowed.load(std::sync::atomic::Ordering::Acquire)) as ResourceFence
        };
        let handle = start_timer(
            &resources,
            owner.clone(),
            MAX_TIMER_MILLISECONDS,
            None,
            fence,
        )
        .unwrap();
        allowed.store(false, std::sync::atomic::Ordering::Release);
        tokio::time::sleep(Duration::from_millis(u64::from(
            MIN_TIMER_MILLISECONDS + 30,
        )))
        .await;
        let (events, _) = resources.take_events(&owner, &handle, 1).unwrap();
        assert!(
            matches!(events.as_slice(), [event] if matches!(event.kind, PluginApiResourceEventKind::Cancelled {}))
        );
        resources.close(&owner, &handle).await.unwrap();
    }
}
