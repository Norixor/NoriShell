//! Admission for long-lived, metadata-only plugin subscriptions.

use std::collections::BTreeSet;

use norishell_core_api::{
    PluginApiErrorCode, PluginApiValue, PluginCapability, PluginSubscriptionTopic,
    PluginTerminalState, SshSessionId, WireSequence,
};

use super::{api_invocation::ApiInvocation, *};
use crate::plugin_api::{
    ResourceFence, ResourceOwner,
    subscriptions::{
        SubscriptionRequest, TerminalSubscriptionBinding, TerminalSubscriptionSession,
        start_subscription,
    },
};

const MAX_SUBSCRIPTION_TOPICS: usize = 16;

impl PluginService {
    pub(super) async fn start_api_subscription(
        &self,
        invocation: &ApiInvocation<'_>,
        installed: &PluginInstalledRecord,
        owner: &ResourceOwner,
        topics: &[PluginSubscriptionTopic],
    ) -> Result<PluginApiValue, PluginApiErrorCode> {
        if topics.is_empty() || topics.len() > MAX_SUBSCRIPTION_TOPICS {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        let mut unique = BTreeSet::new();
        let mut terminal_contexts = Vec::new();
        let mut host_scope = false;
        let mut plugin_settings = false;
        for topic in topics {
            let key = match topic {
                PluginSubscriptionTopic::TerminalContext { context_handle } => {
                    if context_handle.is_empty()
                        || context_handle.len() > 120
                        || context_handle.chars().any(char::is_control)
                    {
                        return Err(PluginApiErrorCode::InvalidRequest);
                    }
                    terminal_contexts.push(context_handle.clone());
                    format!("terminal:{context_handle}")
                }
                PluginSubscriptionTopic::HostScope {} => {
                    host_scope = true;
                    "hostScope".to_owned()
                }
                PluginSubscriptionTopic::PluginSettings {} => {
                    plugin_settings = true;
                    "pluginSettings".to_owned()
                }
            };
            if !unique.insert(key) {
                return Err(PluginApiErrorCode::InvalidRequest);
            }
        }

        let current = self.api_invocation_fence(owner, invocation);
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let terminal_events = self.sessions.subscribe_plugin_metadata();
        let snapshots = if terminal_contexts.is_empty() {
            None
        } else {
            if !installed
                .capabilities
                .contains(&PluginCapability::TerminalMetadata)
            {
                return Err(PluginApiErrorCode::PermissionDenied);
            }
            Some((
                self.sessions
                    .snapshot(invocation.request_id().clone())
                    .await
                    .map_err(|_| PluginApiErrorCode::Unavailable)?,
                self.sessions
                    .local_snapshot(invocation.request_id().clone())
                    .await
                    .map_err(|_| PluginApiErrorCode::Unavailable)?,
            ))
        };

        let terminal_epoch = if terminal_contexts.is_empty() {
            None
        } else {
            Some(
                self.capability_grant_epoch_for_record(
                    invocation.request_id().clone(),
                    installed,
                    PluginCapability::TerminalMetadata,
                )
                .map_err(|_| PluginApiErrorCode::PermissionDenied)?,
            )
        };
        let host_scope_epoch = if host_scope {
            if !installed
                .capabilities
                .contains(&PluginCapability::HostMetadataRead)
            {
                return Err(PluginApiErrorCode::PermissionDenied);
            }
            Some(
                self.capability_grant_epoch_for_record(
                    invocation.request_id().clone(),
                    installed,
                    PluginCapability::HostMetadataRead,
                )
                .map_err(|_| PluginApiErrorCode::PermissionDenied)?,
            )
        } else {
            None
        };
        let mut lifetime_fences = vec![self.api_resource_fence(owner.clone(), None)];
        if invocation.resource_consumer().is_some() {
            lifetime_fences.push(invocation.transaction_authority(self));
        }
        if let Some(epoch) = terminal_epoch {
            lifetime_fences.push(self.api_resource_fence(
                owner.clone(),
                Some((PluginCapability::TerminalMetadata, epoch)),
            ));
        }
        if let Some(epoch) = host_scope_epoch {
            lifetime_fences.push(self.api_resource_fence(
                owner.clone(),
                Some((PluginCapability::HostMetadataRead, epoch)),
            ));
        }
        let resource_fence: ResourceFence =
            Arc::new(move || lifetime_fences.iter().all(|fence| fence()));
        if !current() || !resource_fence() {
            return Err(PluginApiErrorCode::Revoked);
        }

        let mut terminal_bindings = Vec::with_capacity(terminal_contexts.len());
        for context_handle in terminal_contexts {
            terminal_bindings.push(self.resolve_terminal_subscription(
                invocation,
                owner,
                &context_handle,
                &snapshots.as_ref().expect("terminal snapshots").0,
                &snapshots.as_ref().expect("terminal snapshots").1,
            )?);
        }
        let _creation_permit = self
            .api_creation_permit(invocation.request_id().clone())
            .map_err(|_| PluginApiErrorCode::Unavailable)?;
        if !current() || !resource_fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let handle = start_subscription(
            &invocation.resource_registry(&self.api.resources),
            owner.clone(),
            SubscriptionRequest {
                terminal_bindings,
                host_scope,
                plugin_settings,
                terminal_events,
                host_scope_events: self.api.subscriptions.subscribe_host_scope(),
                settings_events: self.api.subscriptions.subscribe_settings(),
                resource_fence,
            },
        )?;
        Ok(PluginApiValue::SubscriptionStarted { handle })
    }

    fn resolve_terminal_subscription(
        &self,
        invocation: &ApiInvocation<'_>,
        owner: &ResourceOwner,
        context_handle: &str,
        snapshot: &norishell_core_api::SshSessionSnapshot,
        local_snapshot: &norishell_core_api::LocalSessionSnapshot,
    ) -> Result<TerminalSubscriptionBinding, PluginApiErrorCode> {
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let active = runtime
            .target_contexts
            .get(context_handle)
            .cloned()
            .filter(|active| active.context.target_id.as_str().starts_with("terminal."))
            .ok_or(PluginApiErrorCode::NotFound)?;
        let issued_to_invocation = match invocation {
            ApiInvocation::Declarative { request, .. } => {
                request.plugin_id == owner.plugin_id
                    && request.signer_fingerprint_sha256 == owner.signer
                    && request.expected_package_sha256 == owner.package
                    && request.instance_generation == owner.generation
                    && request.context_handle.as_str() == context_handle
                    && request.target_id == active.context.target_id
            }
            ApiInvocation::Isolated { .. }
            | ApiInvocation::Provider { .. }
            | ApiInvocation::Task { .. } => false,
        };
        let issued_scoped_context = runtime
            .active_instances
            .get(owner.plugin_id.as_str())
            .filter(|instance| {
                instance.signer_fingerprint_sha256 == owner.signer
                    && instance.package_sha256 == owner.package
                    && instance.instance_generation == owner.generation
            })
            .is_some_and(|instance| instance.scoped_templates.contains_key(context_handle));
        if !issued_to_invocation && !issued_scoped_context {
            return Err(PluginApiErrorCode::PermissionDenied);
        }
        drop(runtime);
        let parts = active.instance_key.split('|').collect::<Vec<_>>();
        if parts.len() != 5 {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        let generation = parts[4]
            .parse::<u64>()
            .map(WireSequence::new)
            .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
        let (session, initial_state, initial_state_revision) = match parts[2] {
            "ssh" => {
                let session_id = SshSessionId::parse(parts[3].to_owned())
                    .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
                let (state, state_revision) = snapshot
                    .sessions
                    .iter()
                    .find(|session| {
                        session.session_id == session_id && session.generation == generation
                    })
                    .map(|session| {
                        (
                            plugin_subscription_terminal_state(session.state),
                            session.state_revision,
                        )
                    })
                    .ok_or(PluginApiErrorCode::NotFound)?;
                (
                    TerminalSubscriptionSession::Ssh(session_id),
                    state,
                    state_revision,
                )
            }
            "local" => {
                let session_id = norishell_core_api::LocalSessionId::parse(parts[3].to_owned())
                    .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
                let (state, state_revision) = local_snapshot
                    .sessions
                    .iter()
                    .find(|session| {
                        session.session_id == session_id && session.generation == generation
                    })
                    .map(|session| {
                        (
                            plugin_subscription_local_terminal_state(session.state),
                            session.state_revision,
                        )
                    })
                    .ok_or(PluginApiErrorCode::NotFound)?;
                (
                    TerminalSubscriptionSession::Local(session_id),
                    state,
                    state_revision,
                )
            }
            _ => return Err(PluginApiErrorCode::InvalidRequest),
        };
        let target_id = active.context.target_id.clone();
        let target_revision = active.context.target_revision;
        let instance_key = active.instance_key.clone();
        let runtime = self.runtime.clone();
        let handle = context_handle.to_owned();
        let context_fence: ResourceFence = Arc::new(move || {
            runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .target_contexts
                .get(&handle)
                .is_some_and(|current| {
                    current.context.target_id == target_id
                        && current.context.target_revision == target_revision
                        && current.instance_key == instance_key
                })
        });
        Ok(TerminalSubscriptionBinding {
            context_handle: context_handle.to_owned(),
            target_id: active.context.target_id,
            session,
            generation,
            initial_state_revision,
            initial_state,
            context_fence,
        })
    }
}

fn plugin_subscription_local_terminal_state(
    state: norishell_core_api::LocalSessionState,
) -> PluginTerminalState {
    use norishell_core_api::LocalSessionState;
    match state {
        LocalSessionState::Starting => PluginTerminalState::Starting,
        LocalSessionState::Running => PluginTerminalState::Running,
        LocalSessionState::Stopping => PluginTerminalState::Closing,
        LocalSessionState::Exited | LocalSessionState::Closed => PluginTerminalState::Closed,
        LocalSessionState::Failed => PluginTerminalState::Failed,
    }
}

fn plugin_subscription_terminal_state(
    state: norishell_core_api::SshSessionState,
) -> PluginTerminalState {
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
