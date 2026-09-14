//! Trusted admission context for declarative and isolated plugin API calls.

use super::*;

pub(super) enum ApiInvocation<'a> {
    Declarative {
        request: &'a PluginUiActionRequest,
        explicit_user_action: bool,
        input_focus: Option<norishell_core_api::TerminalInputFocusSnapshot>,
    },
    Provider {
        request_id: RequestId,
        owner: crate::plugin_api::ResourceOwner,
        operation_id: String,
        authority: crate::plugin_api::ResourceFence,
        transaction_authority: crate::plugin_api::ResourceFence,
        consumer: crate::plugin_api::ResourceConsumer,
        explicit_user_action: bool,
    },
    Task {
        request_id: RequestId,
        owner: crate::plugin_api::ResourceOwner,
        operation_id: String,
        authority: crate::plugin_api::ResourceFence,
        transaction_authority: crate::plugin_api::ResourceFence,
        consumer: crate::plugin_api::ResourceConsumer,
        explicit_user_action: bool,
    },
    Isolated {
        request_id: RequestId,
        owner: crate::plugin_api::ResourceOwner,
        operation_id: String,
        authority: crate::plugin_api::ResourceFence,
        transaction_authority: crate::plugin_api::ResourceFence,
        explicit_user_action: bool,
    },
}

impl<'a> ApiInvocation<'a> {
    pub(super) fn declarative(
        request: &'a PluginUiActionRequest,
        explicit_user_action: bool,
        input_focus: Option<norishell_core_api::TerminalInputFocusSnapshot>,
    ) -> Self {
        Self::Declarative {
            request,
            explicit_user_action,
            input_focus,
        }
    }

    pub(super) fn isolated(
        request_id: RequestId,
        owner: crate::plugin_api::ResourceOwner,
        operation_id: String,
        authority: crate::plugin_api::ResourceFence,
        transaction_authority: crate::plugin_api::ResourceFence,
        explicit_user_action: bool,
    ) -> Self {
        Self::Isolated {
            request_id,
            owner,
            operation_id,
            authority,
            transaction_authority,
            explicit_user_action,
        }
    }

    pub(super) fn request_id(&self) -> &RequestId {
        match self {
            Self::Declarative { request, .. } => &request.meta.request_id,
            Self::Isolated { request_id, .. }
            | Self::Provider { request_id, .. }
            | Self::Task { request_id, .. } => request_id,
        }
    }

    pub(super) fn owner(
        &self,
        installed: &PluginInstalledRecord,
    ) -> crate::plugin_api::ResourceOwner {
        match self {
            Self::Declarative { request, .. } => crate::plugin_api::ResourceOwner {
                plugin_id: installed.plugin_id.clone(),
                signer: installed.signer_fingerprint_sha256.clone(),
                package: installed.package_sha256.clone(),
                generation: request.instance_generation,
            },
            Self::Isolated { owner, .. }
            | Self::Provider { owner, .. }
            | Self::Task { owner, .. } => owner.clone(),
        }
    }

    pub(super) fn operation_identity(&self) -> &str {
        match self {
            Self::Declarative { request, .. } => request.action_id.as_str(),
            Self::Isolated { operation_id, .. }
            | Self::Provider { operation_id, .. }
            | Self::Task { operation_id, .. } => operation_id,
        }
    }

    pub(super) fn matches_installation(&self, installed: &PluginInstalledRecord) -> bool {
        match self {
            Self::Declarative { request, .. } => {
                request.plugin_id == installed.plugin_id
                    && request.signer_fingerprint_sha256 == installed.signer_fingerprint_sha256
                    && request.expected_package_sha256 == installed.package_sha256
                    && request.expected_state_version == installed.state_version
            }
            Self::Isolated { owner, .. }
            | Self::Provider { owner, .. }
            | Self::Task { owner, .. } => {
                owner.plugin_id == installed.plugin_id
                    && owner.signer == installed.signer_fingerprint_sha256
                    && owner.package == installed.package_sha256
            }
        }
    }

    pub(super) fn explicit_user_action(&self) -> bool {
        match self {
            Self::Declarative {
                explicit_user_action,
                ..
            }
            | Self::Task {
                explicit_user_action,
                ..
            }
            | Self::Provider {
                explicit_user_action,
                ..
            }
            | Self::Isolated {
                explicit_user_action,
                ..
            } => *explicit_user_action,
        }
    }

    pub(super) fn input_focus(&self) -> Option<norishell_core_api::TerminalInputFocusSnapshot> {
        match self {
            Self::Declarative { input_focus, .. } => input_focus.clone(),
            Self::Isolated { .. } | Self::Provider { .. } | Self::Task { .. } => None,
        }
    }

    pub(super) fn is_isolated(&self) -> bool {
        matches!(self, Self::Isolated { .. })
    }

    pub(super) fn resource_consumer(&self) -> Option<crate::plugin_api::ResourceConsumer> {
        match self {
            Self::Provider { consumer, .. } | Self::Task { consumer, .. } => Some(consumer.clone()),
            _ => None,
        }
    }

    pub(super) fn resource_registry(
        &self,
        registry: &crate::plugin_api::ResourceRegistry,
    ) -> crate::plugin_api::ResourceRegistry {
        match self {
            Self::Provider { consumer, .. } | Self::Task { consumer, .. } => {
                registry.for_consumer(consumer.clone())
            }
            _ => registry.clone(),
        }
    }

    pub(super) fn transaction_authority(
        &self,
        service: &PluginService,
    ) -> crate::plugin_api::ResourceFence {
        match self {
            Self::Declarative { request, .. } => {
                let service = service.clone();
                let request = (*request).clone();
                Arc::new(move || service.broker_action_runtime_current(&request))
            }
            Self::Task {
                transaction_authority,
                ..
            }
            | Self::Provider {
                transaction_authority,
                ..
            }
            | Self::Isolated {
                transaction_authority,
                ..
            } => transaction_authority.clone(),
        }
    }

    pub(super) fn action_authority(
        &self,
        service: &PluginService,
    ) -> crate::plugin_api::ResourceFence {
        match self {
            Self::Declarative { request, .. } => {
                let service = service.clone();
                let request = (*request).clone();
                Arc::new(move || service.broker_action_current(&request))
            }
            Self::Isolated { authority, .. }
            | Self::Provider { authority, .. }
            | Self::Task { authority, .. } => authority.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_calls_keep_the_core_operation_identity_and_never_inherit_terminal_focus() {
        let invocation = ApiInvocation::isolated(
            RequestId::new(),
            crate::plugin_api::ResourceOwner {
                plugin_id: PluginId::parse("example.plugin").expect("valid plugin id"),
                signer: "signer".to_owned(),
                package: "package".to_owned(),
                generation: WireSequence::new(3),
            },
            "isolated:surface:networkStart".to_owned(),
            Arc::new(|| true),
            Arc::new(|| true),
            true,
        );

        assert_eq!(
            invocation.operation_identity(),
            "isolated:surface:networkStart"
        );
        assert!(invocation.explicit_user_action());
        assert!(invocation.input_focus().is_none());
    }
}
