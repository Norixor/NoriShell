//! One ownership ledger for long-lived plugin resources. Cancellation is not closure.

use std::{
    collections::{BTreeMap, VecDeque},
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};

use norishell_core_api::{
    PluginApiErrorCode, PluginApiResourceEvent, PluginApiResourceEventKind, PluginApiResourceState,
    PluginApiResourceSummary, PluginId, WireSequence,
};
use tokio::{
    sync::{Mutex as AsyncMutex, Notify, mpsc, oneshot, watch},
    task::JoinHandle,
};
use uuid::Uuid;

use super::{MAX_PENDING_EVENTS, MAX_PLUGIN_RESOURCES, MAX_TOTAL_RESOURCES};

pub(crate) const MAX_RESOURCE_EVENT_BYTES: usize = 16 * 1024;

/// Constructed by Core from the active installation, never deserialized from a guest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResourceOwner {
    pub plugin_id: PluginId,
    pub signer: String,
    pub package: String,
    pub generation: WireSequence,
}

/// Private connection identity; never accepted from plugin JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResourceConsumer {
    pub connection: uuid::Uuid,
    pub generation: WireSequence,
    pub stream: uuid::Uuid,
}

struct Resource {
    owner: ResourceOwner,
    consumer: Option<ResourceConsumer>,
    events_ready: Arc<Notify>,
    kind: &'static str,
    cancel: watch::Sender<bool>,
    completion: AsyncMutex<Completion>,
    state: Mutex<PluginApiResourceState>,
    events: Mutex<ResourceEvents>,
    event_space: Notify,
    commands: Option<mpsc::Sender<ResourceCommand>>,
}

#[derive(Default)]
struct ResourceEvents {
    next_sequence: u64,
    events: VecDeque<PluginApiResourceEvent>,
    backpressured: bool,
}

#[derive(Clone)]
pub(crate) struct ResourceEventWriter {
    resource: Arc<Resource>,
}

/// One owner-scoped command. The driver acknowledges only after it accepted or rejected the
/// payload, so a send cannot be reported as delivered after its resource has already closed.
pub(crate) struct ResourceCommand {
    payload: Vec<u8>,
    acknowledgement: oneshot::Sender<Result<(), PluginApiErrorCode>>,
}

impl ResourceCommand {
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub fn finish(self, result: Result<(), PluginApiErrorCode>) {
        let _ = self.acknowledgement.send(result);
    }
}

pub(crate) struct ResourceCommandReceiver {
    receiver: mpsc::Receiver<ResourceCommand>,
}

impl ResourceCommandReceiver {
    pub async fn recv(&mut self) -> Option<ResourceCommand> {
        self.receiver.recv().await
    }

    /// Process drivers run their owned OS handle on a blocking worker so the complete process
    /// tree can be reaped before the registry releases its blocker. This receives only an
    /// already owner-scoped command and preserves the existing per-write acknowledgement.
    pub fn try_recv(&mut self) -> Option<ResourceCommand> {
        self.receiver.try_recv().ok()
    }
}

impl ResourceEventWriter {
    pub fn emit(&self, kind: PluginApiResourceEventKind) -> Result<(), PluginApiErrorCode> {
        let mut events = self
            .resource
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if events.events.len() >= usize::from(MAX_PENDING_EVENTS) {
            events.backpressured = true;
            return Err(PluginApiErrorCode::Busy);
        }
        let sequence = events
            .next_sequence
            .checked_add(1)
            .ok_or(PluginApiErrorCode::Unavailable)?;
        let event = PluginApiResourceEvent {
            sequence: WireSequence::new(sequence),
            kind,
        };
        if serde_json::to_vec(&event).map_or(true, |bytes| bytes.len() > MAX_RESOURCE_EVENT_BYTES) {
            return Err(PluginApiErrorCode::QuotaExceeded);
        }
        events.next_sequence = sequence;
        events.events.push_back(event);
        self.resource.events_ready.notify_waiters();
        Ok(())
    }

    /// Network drivers use this form: bytes are never silently discarded when the guest has not
    /// polled enough events. A cancellation or revoked fence unblocks the producer promptly.
    pub async fn emit_backpressured(
        &self,
        kind: PluginApiResourceEventKind,
        cancel: &mut watch::Receiver<bool>,
        fence: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<(), PluginApiErrorCode> {
        loop {
            if !fence() {
                return Err(PluginApiErrorCode::Revoked);
            }
            let notified = self.resource.event_space.notified();
            let event = {
                let mut events = self
                    .resource
                    .events
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if events.events.len() >= usize::from(MAX_PENDING_EVENTS) {
                    events.backpressured = true;
                    None
                } else {
                    let sequence = events
                        .next_sequence
                        .checked_add(1)
                        .ok_or(PluginApiErrorCode::Unavailable)?;
                    let event = PluginApiResourceEvent {
                        sequence: WireSequence::new(sequence),
                        kind: kind.clone(),
                    };
                    if serde_json::to_vec(&event)
                        .map_or(true, |bytes| bytes.len() > MAX_RESOURCE_EVENT_BYTES)
                    {
                        return Err(PluginApiErrorCode::QuotaExceeded);
                    }
                    events.next_sequence = sequence;
                    events.events.push_back(event);
                    Some(())
                }
            };
            if event.is_some() {
                self.resource.events_ready.notify_waiters();
                return Ok(());
            }
            tokio::select! {
                _ = notified => {}
                changed = cancel.changed() => {
                    if changed.is_err() || *cancel.borrow_and_update() {
                        return Err(PluginApiErrorCode::Cancelled);
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }
    }
}

enum Completion {
    Running(JoinHandle<Result<(), PluginApiErrorCode>>),
    Finished(Result<(), PluginApiErrorCode>),
}

#[derive(Clone, Default)]
pub(crate) struct ResourceRegistry {
    entries: Arc<Mutex<BTreeMap<String, Arc<Resource>>>>,
    consumer: Option<ResourceConsumer>,
    events_ready: Arc<Notify>,
}

impl ResourceRegistry {
    /// A scoped view shares quota and cleanup ownership but cannot access another consumer.
    /// Drivers receive this view before reservation, including their very first output event.
    pub fn for_consumer(&self, consumer: ResourceConsumer) -> Self {
        Self {
            entries: self.entries.clone(),
            consumer: Some(consumer),
            events_ready: self.events_ready.clone(),
        }
    }

    /// Register before checking queues, so a producer between inspection and await cannot be lost.
    pub async fn wait_events(&self, owner: &ResourceOwner) {
        loop {
            let notified = self.events_ready.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let pending = self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .values()
                .any(|resource| {
                    resource.owner == *owner
                        && resource.consumer == self.consumer
                        && !resource
                            .events
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .events
                            .is_empty()
                });
            if pending {
                return;
            }
            notified.await;
        }
    }

    /// Reserve the quota before starting work. The driver must acknowledge cancellation
    /// only after all of its child I/O and processes have stopped.
    pub fn spawn<F, Fut>(
        &self,
        owner: ResourceOwner,
        kind: &'static str,
        driver: F,
    ) -> Result<String, PluginApiErrorCode>
    where
        F: FnOnce(watch::Receiver<bool>, ResourceEventWriter) -> Fut,
        Fut: Future<Output = Result<(), PluginApiErrorCode>> + Send + 'static,
    {
        self.spawn_inner(
            owner,
            kind,
            None,
            move |cancel, events, _commands| driver(cancel, events),
            None,
        )
    }

    /// A command channel is reserved for duplex resources such as TCP, UDP, TLS and WebSocket.
    /// It remains in the registry, where Core can enforce the exact resource owner before any
    /// bytes reach the driver.
    pub fn spawn_with_commands<F, Fut>(
        &self,
        owner: ResourceOwner,
        kind: &'static str,
        driver: F,
    ) -> Result<String, PluginApiErrorCode>
    where
        F: FnOnce(watch::Receiver<bool>, ResourceEventWriter, ResourceCommandReceiver) -> Fut,
        Fut: Future<Output = Result<(), PluginApiErrorCode>> + Send + 'static,
    {
        let (sender, command_receiver) = mpsc::channel(16);
        self.spawn_inner(
            owner,
            kind,
            Some(sender),
            move |cancel, events, commands| {
                driver(cancel, events, commands.expect("network command receiver"))
            },
            Some(command_receiver),
        )
    }

    fn spawn_inner<F, Fut>(
        &self,
        owner: ResourceOwner,
        kind: &'static str,
        commands: Option<mpsc::Sender<ResourceCommand>>,
        driver: F,
        command_receiver: Option<mpsc::Receiver<ResourceCommand>>,
    ) -> Result<String, PluginApiErrorCode>
    where
        F: FnOnce(
            watch::Receiver<bool>,
            ResourceEventWriter,
            Option<ResourceCommandReceiver>,
        ) -> Fut,
        Fut: Future<Output = Result<(), PluginApiErrorCode>> + Send + 'static,
    {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if entries.len() >= MAX_TOTAL_RESOURCES
            || entries
                .values()
                .filter(|resource| resource.owner.plugin_id == owner.plugin_id)
                .count()
                >= usize::from(MAX_PLUGIN_RESOURCES)
        {
            return Err(PluginApiErrorCode::QuotaExceeded);
        }
        let (cancel, cancel_receiver) = watch::channel(false);
        let handle = Uuid::new_v4().to_string();
        let resource = Arc::new(Resource {
            owner,
            consumer: self.consumer.clone(),
            events_ready: self.events_ready.clone(),
            kind,
            cancel,
            completion: AsyncMutex::new(Completion::Finished(Ok(()))),
            state: Mutex::new(PluginApiResourceState::Open),
            events: Mutex::new(ResourceEvents::default()),
            event_space: Notify::new(),
            commands,
        });
        let writer = ResourceEventWriter {
            resource: resource.clone(),
        };
        let task = tokio::spawn(driver(
            cancel_receiver,
            writer,
            command_receiver.map(|receiver| ResourceCommandReceiver { receiver }),
        ));
        *resource
            .completion
            .try_lock()
            .expect("new resource completion is not shared") = Completion::Running(task);
        entries.insert(handle.clone(), resource);
        Ok(handle)
    }

    pub fn list(&self, owner: &ResourceOwner) -> Vec<PluginApiResourceSummary> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|(_, resource)| resource.owner == *owner && resource.consumer == self.consumer)
            .map(|(handle, resource)| PluginApiResourceSummary {
                handle: handle.clone(),
                generation: resource.owner.generation,
                resource_kind: resource.kind.to_owned(),
                state: *resource
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            })
            .collect()
    }

    pub fn exit_blockers(&self) -> Vec<norishell_core_api::ExitBlocker> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(
                |(id, resource)| norishell_core_api::ExitBlocker::PluginResource {
                    plugin_id: resource.owner.plugin_id.clone(),
                    resource_kind: resource.kind.to_owned(),
                    resource_id: id.clone(),
                },
            )
            .collect()
    }

    pub fn plugin_ids(&self) -> Vec<PluginId> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .map(|resource| resource.owner.plugin_id.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub async fn close(
        &self,
        owner: &ResourceOwner,
        handle: &str,
    ) -> Result<(), PluginApiErrorCode> {
        let resource = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(handle)
            .filter(|resource| resource.owner == *owner && resource.consumer == self.consumer)
            .cloned()
            .ok_or(PluginApiErrorCode::NotFound)?;
        self.close_resource(handle, resource, Duration::from_secs(3))
            .await
    }

    pub fn take_events(
        &self,
        owner: &ResourceOwner,
        handle: &str,
        limit: u16,
    ) -> Result<(Vec<PluginApiResourceEvent>, bool), PluginApiErrorCode> {
        if limit == 0 || limit > MAX_PENDING_EVENTS {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        let resource = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(handle)
            .filter(|resource| resource.owner == *owner && resource.consumer == self.consumer)
            .cloned()
            .ok_or(PluginApiErrorCode::NotFound)?;
        let mut events = resource
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let taken = (0..limit)
            .filter_map(|_| events.events.pop_front())
            .collect::<Vec<_>>();
        let backpressured = std::mem::take(&mut events.backpressured);
        if !taken.is_empty() {
            resource.event_space.notify_waiters();
        }
        Ok((taken, backpressured))
    }

    /// Waits for the resource driver to acknowledge one owner-scoped outbound payload. This is a
    /// delivery handoff, not a claim that remote peer received the bytes.
    pub async fn send(
        &self,
        owner: &ResourceOwner,
        handle: &str,
        payload: Vec<u8>,
        fence: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<(), PluginApiErrorCode> {
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let (sender, mut cancel) = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(handle)
            .filter(|resource| resource.owner == *owner && resource.consumer == self.consumer)
            .filter(|resource| {
                *resource
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    == PluginApiResourceState::Open
            })
            .and_then(|resource| {
                resource
                    .commands
                    .clone()
                    .map(|sender| (sender, resource.cancel.subscribe()))
            })
            .ok_or(PluginApiErrorCode::NotFound)?;
        let (acknowledgement, received) = oneshot::channel();
        let send = sender.send(ResourceCommand {
            payload,
            acknowledgement,
        });
        tokio::pin!(send);
        let enqueue_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            if !fence() {
                return Err(PluginApiErrorCode::Revoked);
            }
            let remaining = enqueue_deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(PluginApiErrorCode::Busy);
            }
            tokio::select! {
                result = &mut send => {
                    result.map_err(|_| PluginApiErrorCode::Cancelled)?;
                    break;
                }
                changed = cancel.changed() => {
                    if changed.is_err() || *cancel.borrow_and_update() {
                        return Err(PluginApiErrorCode::Cancelled);
                    }
                }
                _ = tokio::time::sleep(remaining.min(Duration::from_millis(100))) => {}
            }
        }
        let mut received = received;
        let acknowledgement_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if !fence() {
                return Err(PluginApiErrorCode::OutcomeUnknown);
            }
            let remaining =
                acknowledgement_deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(PluginApiErrorCode::OutcomeUnknown);
            }
            tokio::select! {
                result = &mut received => return result.map_err(|_| PluginApiErrorCode::Cancelled)?,
                changed = cancel.changed() => {
                    if changed.is_err() || *cancel.borrow_and_update() {
                        return Err(PluginApiErrorCode::OutcomeUnknown);
                    }
                }
                _ = tokio::time::sleep(remaining.min(Duration::from_millis(100))) => {}
            }
        }
    }

    pub async fn stop_plugin(&self, plugin_id: &PluginId) -> Result<(), PluginApiErrorCode> {
        self.stop(plugin_id, None).await
    }

    pub async fn stop_plugin_generation(
        &self,
        plugin_id: &PluginId,
        generation: WireSequence,
    ) -> Result<(), PluginApiErrorCode> {
        self.stop(plugin_id, Some(generation)).await
    }

    async fn stop(
        &self,
        plugin_id: &PluginId,
        generation: Option<WireSequence>,
    ) -> Result<(), PluginApiErrorCode> {
        let resources: Vec<_> = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|(_, resource)| {
                resource.owner.plugin_id == *plugin_id
                    && (self.consumer.is_none() || resource.consumer == self.consumer)
                    && generation.is_none_or(|generation| resource.owner.generation == generation)
            })
            .map(|(handle, resource)| (handle.clone(), resource.clone()))
            .collect();
        // Revoke every owner before awaiting any one driver.
        for (_, resource) in &resources {
            resource.cancel.send_replace(true);
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        let mut failed = false;
        for (handle, resource) in resources {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            failed |= self
                .close_resource(&handle, resource, remaining)
                .await
                .is_err();
        }
        if failed {
            Err(PluginApiErrorCode::CleanupIncomplete)
        } else {
            Ok(())
        }
    }

    async fn close_resource(
        &self,
        handle: &str,
        resource: Arc<Resource>,
        timeout: Duration,
    ) -> Result<(), PluginApiErrorCode> {
        resource.cancel.send_replace(true);
        *resource
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = PluginApiResourceState::Closing;
        let result = tokio::time::timeout(timeout, async {
            let mut completion = resource.completion.lock().await;
            let result = match &mut *completion {
                Completion::Running(task) => task
                    .await
                    .unwrap_or(Err(PluginApiErrorCode::CleanupIncomplete)),
                Completion::Finished(result) => return *result,
            };
            *completion = Completion::Finished(result);
            result
        })
        .await;
        if !matches!(result, Ok(Ok(()))) {
            *resource
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                PluginApiResourceState::CleanupIncomplete;
            return Err(PluginApiErrorCode::CleanupIncomplete);
        }
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if entries
            .get(handle)
            .is_some_and(|current| Arc::ptr_eq(current, &resource))
        {
            entries.remove(handle);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner() -> ResourceOwner {
        ResourceOwner {
            plugin_id: PluginId::parse("org.norishell.resource-test").unwrap(),
            signer: "a".repeat(64),
            package: "b".repeat(64),
            generation: WireSequence::new(1),
        }
    }

    #[tokio::test]
    async fn provider_consumers_isolate_events_sends_and_cleanup() {
        let registry = ResourceRegistry::default();
        let owner = owner();
        let consumer = ResourceConsumer {
            connection: Uuid::new_v4(),
            generation: WireSequence::new(1),
            stream: Uuid::new_v4(),
        };
        let first = registry.for_consumer(consumer.clone());
        let second = registry.for_consumer(ResourceConsumer {
            stream: Uuid::new_v4(),
            ..consumer
        });
        let handle = first.spawn_with_commands(owner.clone(), "provider", |mut cancel, events, mut commands| async move {
            events.emit(PluginApiResourceEventKind::TimerFired {})?;
            tokio::select! {
                _ = cancel.changed() => {},
                command = commands.recv() => { if let Some(command) = command { command.finish(Ok(())); } }
            }
            Ok(())
        }).unwrap();
        tokio::time::timeout(Duration::from_secs(1), first.wait_events(&owner))
            .await
            .unwrap();
        assert!(registry.list(&owner).is_empty());
        assert!(second.list(&owner).is_empty());
        assert!(matches!(
            registry.take_events(&owner, &handle, 1),
            Err(PluginApiErrorCode::NotFound)
        ));
        assert_eq!(
            second.send(&owner, &handle, vec![1], &|| true).await,
            Err(PluginApiErrorCode::NotFound)
        );
        assert_eq!(
            registry.close(&owner, &handle).await,
            Err(PluginApiErrorCode::NotFound)
        );
        second
            .stop_plugin_generation(&owner.plugin_id, owner.generation)
            .await
            .unwrap();
        assert_eq!(first.list(&owner).len(), 1);
        assert_eq!(first.take_events(&owner, &handle, 1).unwrap().0.len(), 1);
        // Global plugin shutdown also owns resources hidden from ordinary plugin calls.
        registry.stop_plugin(&owner.plugin_id).await.unwrap();
        assert!(first.list(&owner).is_empty());
    }

    #[tokio::test]
    async fn foreign_owner_and_old_generation_cannot_close_a_resource() {
        let registry = ResourceRegistry::default();
        let owner = owner();
        let handle = registry
            .spawn(owner.clone(), "test", |mut cancel, _events| async move {
                while !*cancel.borrow_and_update() {
                    if cancel.changed().await.is_err() {
                        break;
                    }
                }
                Ok(())
            })
            .unwrap();
        let mut other = owner.clone();
        other.generation = WireSequence::new(2);
        assert_eq!(
            registry.close(&other, &handle).await,
            Err(PluginApiErrorCode::NotFound)
        );
        other = owner.clone();
        other.package = "c".repeat(64);
        assert!(registry.list(&other).is_empty());
        assert_eq!(
            registry.close(&other, &handle).await,
            Err(PluginApiErrorCode::NotFound)
        );
        registry.close(&owner, &handle).await.unwrap();
        assert!(registry.plugin_ids().is_empty());
    }

    #[tokio::test]
    async fn timeout_retains_driver_and_can_be_retried_after_real_completion() {
        let registry = ResourceRegistry::default();
        let owner = owner();
        let (finish, finished) = tokio::sync::oneshot::channel();
        let handle = registry
            .spawn(owner.clone(), "test", |_, _events| async move {
                let _ = finished.await;
                Ok(())
            })
            .unwrap();
        let resource = registry
            .entries
            .lock()
            .unwrap()
            .get(&handle)
            .unwrap()
            .clone();
        assert_eq!(
            registry
                .close_resource(&handle, resource, Duration::from_millis(1))
                .await,
            Err(PluginApiErrorCode::CleanupIncomplete)
        );
        assert_eq!(
            registry.list(&owner)[0].state,
            PluginApiResourceState::CleanupIncomplete
        );
        finish.send(()).unwrap();
        registry.close(&owner, &handle).await.unwrap();
        assert!(registry.list(&owner).is_empty());
    }

    #[tokio::test]
    async fn failed_driver_does_not_block_other_cleanup_or_get_polled_twice() {
        let registry = ResourceRegistry::default();
        let owner = owner();
        let failed = registry
            .spawn(owner.clone(), "failed", |_, _events| async {
                Err(PluginApiErrorCode::CleanupIncomplete)
            })
            .unwrap();
        registry
            .spawn(owner.clone(), "closed", |mut cancel, _events| async move {
                while !*cancel.borrow_and_update() {
                    if cancel.changed().await.is_err() {
                        break;
                    }
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(
            registry.stop_plugin(&owner.plugin_id).await,
            Err(PluginApiErrorCode::CleanupIncomplete)
        );
        assert_eq!(registry.list(&owner).len(), 1);
        assert_eq!(
            registry.close(&owner, &failed).await,
            Err(PluginApiErrorCode::CleanupIncomplete)
        );
    }

    #[tokio::test]
    async fn events_are_owner_scoped_taken_in_order_and_report_backpressure() {
        let registry = ResourceRegistry::default();
        let owner = owner();
        let handle = registry
            .spawn(owner.clone(), "timer", |mut cancel, events| async move {
                for _ in 0..=usize::from(MAX_PENDING_EVENTS) {
                    let _ = events.emit(PluginApiResourceEventKind::TimerFired {});
                }
                while !*cancel.borrow_and_update() {
                    if cancel.changed().await.is_err() {
                        break;
                    }
                }
                Ok(())
            })
            .expect("spawn timer");
        tokio::task::yield_now().await;
        let mut foreign = owner.clone();
        foreign.generation = WireSequence::new(2);
        assert_eq!(
            registry.take_events(&foreign, &handle, 1),
            Err(PluginApiErrorCode::NotFound)
        );
        let (events, backpressured) = registry
            .take_events(&owner, &handle, MAX_PENDING_EVENTS)
            .expect("owner events");
        assert_eq!(events.len(), usize::from(MAX_PENDING_EVENTS));
        assert!(backpressured);
        assert!(
            events
                .windows(2)
                .all(|pair| pair[0].sequence < pair[1].sequence)
        );
        assert!(
            registry
                .take_events(&owner, &handle, 1)
                .unwrap()
                .0
                .is_empty()
        );
        registry.close(&owner, &handle).await.unwrap();
    }
}
