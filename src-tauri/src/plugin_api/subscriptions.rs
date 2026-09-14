//! Typed, metadata-only event subscriptions. Producers never accept guest identifiers;
//! the service broker resolves every terminal context before constructing a binding.

use std::time::Duration;

use norishell_core_api::{
    LocalSessionId, PluginApiErrorCode, PluginApiResourceEventKind, PluginExtensionTargetId,
    PluginId, PluginSubscriptionEvent, PluginTerminalState, SshSessionId, WireSequence,
};
use tokio::sync::{broadcast, watch};

use super::{ResourceEventWriter, ResourceFence, ResourceOwner, ResourceRegistry};
use crate::ssh_session_service::PluginSessionMetadataEvent;

const SOURCE_BUFFER: usize = 64;
const FENCE_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HostScopeSourceEvent {
    pub plugin_id: PluginId,
    pub signer: String,
    pub scope_state_version: Option<WireSequence>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PluginSettingsSourceEvent {
    pub plugin_id: PluginId,
    pub revision: WireSequence,
}

/// Internal typed sources, never a guest-writable event bus.
#[derive(Clone)]
pub(crate) struct SubscriptionSources {
    host_scope: broadcast::Sender<HostScopeSourceEvent>,
    settings: broadcast::Sender<PluginSettingsSourceEvent>,
}

impl Default for SubscriptionSources {
    fn default() -> Self {
        let (host_scope, _) = broadcast::channel(SOURCE_BUFFER);
        let (settings, _) = broadcast::channel(SOURCE_BUFFER);
        Self {
            host_scope,
            settings,
        }
    }
}

impl SubscriptionSources {
    pub(crate) fn subscribe_host_scope(&self) -> broadcast::Receiver<HostScopeSourceEvent> {
        self.host_scope.subscribe()
    }

    pub(crate) fn subscribe_settings(&self) -> broadcast::Receiver<PluginSettingsSourceEvent> {
        self.settings.subscribe()
    }

    pub(crate) fn publish_host_scope(&self, event: HostScopeSourceEvent) {
        let _ = self.host_scope.send(event);
    }

    pub(crate) fn publish_settings(&self, event: PluginSettingsSourceEvent) {
        let _ = self.settings.send(event);
    }
}

/// Core-resolved terminal binding. Its fence proves that the opaque handle still
/// denotes the exact target and session generation resolved at subscription start.
pub(crate) struct TerminalSubscriptionBinding {
    pub context_handle: String,
    pub target_id: PluginExtensionTargetId,
    pub session: TerminalSubscriptionSession,
    pub generation: WireSequence,
    pub initial_state_revision: WireSequence,
    pub initial_state: PluginTerminalState,
    pub context_fence: ResourceFence,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TerminalSubscriptionSession {
    Ssh(SshSessionId),
    Local(LocalSessionId),
}

pub(crate) struct SubscriptionRequest {
    pub terminal_bindings: Vec<TerminalSubscriptionBinding>,
    pub host_scope: bool,
    pub plugin_settings: bool,
    pub terminal_events: broadcast::Receiver<PluginSessionMetadataEvent>,
    pub host_scope_events: broadcast::Receiver<HostScopeSourceEvent>,
    pub settings_events: broadcast::Receiver<PluginSettingsSourceEvent>,
    pub resource_fence: ResourceFence,
}

pub(crate) fn start_subscription(
    resources: &ResourceRegistry,
    owner: ResourceOwner,
    request: SubscriptionRequest,
) -> Result<String, PluginApiErrorCode> {
    if !(request.resource_fence)() {
        return Err(PluginApiErrorCode::Revoked);
    }
    resources.spawn(
        owner.clone(),
        "subscription",
        move |cancel, events| async move {
            match drive_subscription(cancel, events, owner, request).await {
                Err(PluginApiErrorCode::Cancelled | PluginApiErrorCode::Revoked) => Ok(()),
                result => result,
            }
        },
    )
}

async fn drive_subscription(
    mut cancel: watch::Receiver<bool>,
    events: ResourceEventWriter,
    owner: ResourceOwner,
    mut request: SubscriptionRequest,
) -> Result<(), PluginApiErrorCode> {
    let mut terminal_active = vec![true; request.terminal_bindings.len()];
    for (index, binding) in request.terminal_bindings.iter().enumerate() {
        if !(binding.context_fence)() {
            emit_subscription(
                &events,
                &mut cancel,
                request.resource_fence.as_ref(),
                PluginSubscriptionEvent::TerminalContextEnded {
                    context_handle: binding.context_handle.clone(),
                    generation: binding.generation,
                },
            )
            .await?;
            terminal_active[index] = false;
            continue;
        }
        let event_fence = combined_fence(&request.resource_fence, &binding.context_fence);
        let initial = emit_subscription(
            &events,
            &mut cancel,
            event_fence.as_ref(),
            PluginSubscriptionEvent::TerminalContextChanged {
                context_handle: binding.context_handle.clone(),
                target_id: binding.target_id.clone(),
                generation: binding.generation,
                state: binding.initial_state,
            },
        )
        .await;
        if initial == Err(PluginApiErrorCode::Revoked) && (request.resource_fence)() {
            emit_subscription(
                &events,
                &mut cancel,
                request.resource_fence.as_ref(),
                PluginSubscriptionEvent::TerminalContextEnded {
                    context_handle: binding.context_handle.clone(),
                    generation: binding.generation,
                },
            )
            .await?;
            terminal_active[index] = false;
            continue;
        }
        initial?;
        if matches!(
            binding.initial_state,
            PluginTerminalState::Closed | PluginTerminalState::Failed
        ) {
            emit_subscription(
                &events,
                &mut cancel,
                request.resource_fence.as_ref(),
                PluginSubscriptionEvent::TerminalContextEnded {
                    context_handle: binding.context_handle.clone(),
                    generation: binding.generation,
                },
            )
            .await?;
            terminal_active[index] = false;
        }
    }
    loop {
        if *cancel.borrow_and_update() || !(request.resource_fence)() {
            return Ok(());
        }
        reconcile_closed_contexts(
            &events,
            &mut cancel,
            request.resource_fence.as_ref(),
            &request.terminal_bindings,
            &mut terminal_active,
        )
        .await?;
        if !request.host_scope && !request.plugin_settings && !terminal_active.iter().any(|v| *v) {
            return Ok(());
        }

        tokio::select! {
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow_and_update() { return Ok(()); }
            }
            received = request.terminal_events.recv(), if terminal_active.iter().any(|value| *value) => {
                match received {
                    Ok(source) => {
                        for (index, binding) in request.terminal_bindings.iter().enumerate() {
                            let (matches_session, source_generation, state_revision, state) = match (&binding.session, &source) {
                                (TerminalSubscriptionSession::Ssh(expected), PluginSessionMetadataEvent::Ssh { session_id, generation, state_revision, state }) if expected == session_id => (true, *generation, *state_revision, ssh_terminal_state(*state)),
                                (TerminalSubscriptionSession::Local(expected), PluginSessionMetadataEvent::Local { session_id, generation, state_revision, state }) if expected == session_id => (true, *generation, *state_revision, local_terminal_state(*state)),
                                _ => (false, binding.generation, binding.initial_state_revision, binding.initial_state),
                            };
                            if !terminal_active[index] || !matches_session { continue; }
                            if source_generation != binding.generation {
                                emit_subscription(&events, &mut cancel, request.resource_fence.as_ref(),
                                    PluginSubscriptionEvent::TerminalContextEnded {
                                        context_handle: binding.context_handle.clone(),
                                        generation: binding.generation,
                                    }).await?;
                                terminal_active[index] = false;
                                continue;
                            }
                            if state_revision.get() <= binding.initial_state_revision.get() {
                                continue;
                            }
                            if !(binding.context_fence)() {
                                emit_subscription(&events, &mut cancel, request.resource_fence.as_ref(),
                                    PluginSubscriptionEvent::TerminalContextEnded {
                                        context_handle: binding.context_handle.clone(),
                                        generation: binding.generation,
                                    }).await?;
                                terminal_active[index] = false;
                                continue;
                            }
                            let event_fence = combined_fence(&request.resource_fence, &binding.context_fence);
                            let changed = emit_subscription(&events, &mut cancel, event_fence.as_ref(),
                                PluginSubscriptionEvent::TerminalContextChanged {
                                    context_handle: binding.context_handle.clone(),
                                    target_id: binding.target_id.clone(),
                                    generation: binding.generation,
                                    state,
                                }).await;
                            if changed == Err(PluginApiErrorCode::Revoked) && (request.resource_fence)() {
                                emit_subscription(&events, &mut cancel, request.resource_fence.as_ref(),
                                    PluginSubscriptionEvent::TerminalContextEnded {
                                        context_handle: binding.context_handle.clone(),
                                        generation: binding.generation,
                                    }).await?;
                                terminal_active[index] = false;
                                continue;
                            }
                            changed?;
                            if matches!(state, PluginTerminalState::Closed | PluginTerminalState::Failed) {
                                emit_subscription(&events, &mut cancel, request.resource_fence.as_ref(),
                                    PluginSubscriptionEvent::TerminalContextEnded {
                                        context_handle: binding.context_handle.clone(),
                                        generation: binding.generation,
                                    }).await?;
                                terminal_active[index] = false;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => return Err(PluginApiErrorCode::Busy),
                    Err(broadcast::error::RecvError::Closed) => return Err(PluginApiErrorCode::Unavailable),
                }
            }
            received = request.host_scope_events.recv(), if request.host_scope => {
                match received {
                    Ok(source) if source.plugin_id == owner.plugin_id && source.signer == owner.signer => {
                        emit_subscription(&events, &mut cancel, request.resource_fence.as_ref(),
                            PluginSubscriptionEvent::HostScopeChanged { scope_state_version: source.scope_state_version }).await?;
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => return Err(PluginApiErrorCode::Busy),
                    Err(broadcast::error::RecvError::Closed) => return Err(PluginApiErrorCode::Unavailable),
                }
            }
            received = request.settings_events.recv(), if request.plugin_settings => {
                match received {
                    Ok(source) if source.plugin_id == owner.plugin_id => {
                        emit_subscription(&events, &mut cancel, request.resource_fence.as_ref(),
                            PluginSubscriptionEvent::PluginSettingsChanged { revision: source.revision }).await?;
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => return Err(PluginApiErrorCode::Busy),
                    Err(broadcast::error::RecvError::Closed) => return Err(PluginApiErrorCode::Unavailable),
                }
            }
            _ = tokio::time::sleep(FENCE_POLL_INTERVAL) => {}
        }
    }
}

async fn reconcile_closed_contexts(
    events: &ResourceEventWriter,
    cancel: &mut watch::Receiver<bool>,
    resource_fence: &(dyn Fn() -> bool + Send + Sync),
    bindings: &[TerminalSubscriptionBinding],
    active: &mut [bool],
) -> Result<(), PluginApiErrorCode> {
    for (index, binding) in bindings.iter().enumerate() {
        if active[index] && !(binding.context_fence)() {
            emit_subscription(
                events,
                cancel,
                resource_fence,
                PluginSubscriptionEvent::TerminalContextEnded {
                    context_handle: binding.context_handle.clone(),
                    generation: binding.generation,
                },
            )
            .await?;
            active[index] = false;
        }
    }
    Ok(())
}

async fn emit_subscription(
    events: &ResourceEventWriter,
    cancel: &mut watch::Receiver<bool>,
    fence: &(dyn Fn() -> bool + Send + Sync),
    event: PluginSubscriptionEvent,
) -> Result<(), PluginApiErrorCode> {
    events
        .emit_backpressured(
            PluginApiResourceEventKind::Subscription { event },
            cancel,
            fence,
        )
        .await
}

fn combined_fence(left: &ResourceFence, right: &ResourceFence) -> ResourceFence {
    let left = left.clone();
    let right = right.clone();
    std::sync::Arc::new(move || left() && right())
}

fn ssh_terminal_state(state: norishell_core_api::SshSessionState) -> PluginTerminalState {
    use norishell_core_api::SshSessionState;
    match state {
        SshSessionState::Resolving
        | SshSessionState::Connecting
        | SshSessionState::Authenticating
        | SshSessionState::OpeningChannel
        | SshSessionState::AutomatingLogin => PluginTerminalState::Starting,
        SshSessionState::VerifyingHostKey | SshSessionState::AwaitingHostKeyDecision => {
            PluginTerminalState::AwaitingUser
        }
        SshSessionState::Running => PluginTerminalState::Running,
        SshSessionState::Disconnecting => PluginTerminalState::Closing,
        SshSessionState::Closed => PluginTerminalState::Closed,
        SshSessionState::Failed => PluginTerminalState::Failed,
    }
}

fn local_terminal_state(state: norishell_core_api::LocalSessionState) -> PluginTerminalState {
    use norishell_core_api::LocalSessionState;
    match state {
        LocalSessionState::Starting => PluginTerminalState::Starting,
        LocalSessionState::Running => PluginTerminalState::Running,
        LocalSessionState::Stopping => PluginTerminalState::Closing,
        LocalSessionState::Exited | LocalSessionState::Closed => PluginTerminalState::Closed,
        LocalSessionState::Failed => PluginTerminalState::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    fn owner() -> ResourceOwner {
        ResourceOwner {
            plugin_id: PluginId::parse("org.norishell.subscription-test").unwrap(),
            signer: "a".repeat(64),
            package: "b".repeat(64),
            generation: WireSequence::new(1),
        }
    }

    #[tokio::test]
    async fn filters_foreign_sources_and_ends_before_a_new_session_generation() {
        let resources = ResourceRegistry::default();
        let sources = SubscriptionSources::default();
        let owner = owner();
        let (terminal_source, _) = broadcast::channel(8);
        let session_id = SshSessionId::new();
        let request = SubscriptionRequest {
            terminal_bindings: vec![TerminalSubscriptionBinding {
                context_handle: "opaque-context".to_owned(),
                target_id: PluginExtensionTargetId::parse("terminal.tools").unwrap(),
                session: TerminalSubscriptionSession::Ssh(session_id.clone()),
                generation: WireSequence::new(4),
                initial_state_revision: WireSequence::new(7),
                initial_state: PluginTerminalState::Starting,
                context_fence: Arc::new(|| true),
            }],
            host_scope: true,
            plugin_settings: true,
            terminal_events: terminal_source.subscribe(),
            host_scope_events: sources.subscribe_host_scope(),
            settings_events: sources.subscribe_settings(),
            resource_fence: Arc::new(|| true),
        };
        let handle = start_subscription(&resources, owner.clone(), request).unwrap();
        sources.publish_settings(PluginSettingsSourceEvent {
            plugin_id: PluginId::parse("org.norishell.other").unwrap(),
            revision: WireSequence::new(99),
        });
        sources.publish_host_scope(HostScopeSourceEvent {
            plugin_id: owner.plugin_id.clone(),
            signer: "c".repeat(64),
            scope_state_version: Some(WireSequence::new(99)),
        });
        terminal_source
            .send(PluginSessionMetadataEvent::Ssh {
                session_id: session_id.clone(),
                generation: WireSequence::new(4),
                state_revision: WireSequence::new(7),
                state: norishell_core_api::SshSessionState::Connecting,
            })
            .unwrap();
        terminal_source
            .send(PluginSessionMetadataEvent::Ssh {
                session_id: session_id.clone(),
                generation: WireSequence::new(4),
                state_revision: WireSequence::new(8),
                state: norishell_core_api::SshSessionState::Running,
            })
            .unwrap();
        terminal_source
            .send(PluginSessionMetadataEvent::Ssh {
                session_id,
                generation: WireSequence::new(5),
                state_revision: WireSequence::new(9),
                state: norishell_core_api::SshSessionState::Connecting,
            })
            .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        let (events, backpressured) = resources.take_events(&owner, &handle, 8).unwrap();
        assert!(!backpressured);
        assert_eq!(events.len(), 3);
        assert!(
            matches!(events[0].kind, PluginApiResourceEventKind::Subscription { event: PluginSubscriptionEvent::TerminalContextChanged { generation, state: PluginTerminalState::Starting, .. } } if generation == WireSequence::new(4))
        );
        assert!(
            matches!(events[1].kind, PluginApiResourceEventKind::Subscription { event: PluginSubscriptionEvent::TerminalContextChanged { generation, state: PluginTerminalState::Running, .. } } if generation == WireSequence::new(4))
        );
        assert!(
            matches!(events[2].kind, PluginApiResourceEventKind::Subscription { event: PluginSubscriptionEvent::TerminalContextEnded { generation, .. } } if generation == WireSequence::new(4))
        );
        resources.close(&owner, &handle).await.unwrap();
    }

    #[tokio::test]
    async fn local_context_ignores_buffered_state_and_never_emits_after_scope_loss() {
        let resources = ResourceRegistry::default();
        let sources = SubscriptionSources::default();
        let owner = owner();
        let (terminal_source, _) = broadcast::channel(8);
        let session_id = LocalSessionId::new();
        let context_current = Arc::new(AtomicBool::new(true));
        let request = SubscriptionRequest {
            terminal_bindings: vec![TerminalSubscriptionBinding {
                context_handle: "local-context".to_owned(),
                target_id: PluginExtensionTargetId::parse("terminal.footer").unwrap(),
                session: TerminalSubscriptionSession::Local(session_id.clone()),
                generation: WireSequence::new(2),
                initial_state_revision: WireSequence::new(5),
                initial_state: PluginTerminalState::Starting,
                context_fence: {
                    let current = context_current.clone();
                    Arc::new(move || current.load(Ordering::Acquire))
                },
            }],
            host_scope: false,
            plugin_settings: false,
            terminal_events: terminal_source.subscribe(),
            host_scope_events: sources.subscribe_host_scope(),
            settings_events: sources.subscribe_settings(),
            resource_fence: Arc::new(|| true),
        };
        let handle = start_subscription(&resources, owner.clone(), request).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let (initial, _) = resources.take_events(&owner, &handle, 2).unwrap();
        assert_eq!(initial.len(), 1);

        terminal_source
            .send(PluginSessionMetadataEvent::Local {
                session_id: session_id.clone(),
                generation: WireSequence::new(2),
                state_revision: WireSequence::new(5),
                state: norishell_core_api::LocalSessionState::Running,
            })
            .unwrap();
        context_current.store(false, Ordering::Release);
        terminal_source
            .send(PluginSessionMetadataEvent::Local {
                session_id,
                generation: WireSequence::new(2),
                state_revision: WireSequence::new(6),
                state: norishell_core_api::LocalSessionState::Running,
            })
            .unwrap();
        tokio::time::sleep(FENCE_POLL_INTERVAL + Duration::from_millis(30)).await;

        let (events, _) = resources.take_events(&owner, &handle, 3).unwrap();
        assert!(matches!(events.as_slice(), [event] if matches!(event.kind,
            PluginApiResourceEventKind::Subscription {
                event: PluginSubscriptionEvent::TerminalContextEnded { generation, .. }
            } if generation == WireSequence::new(2)
        )));
        resources.close(&owner, &handle).await.unwrap();
    }
}
