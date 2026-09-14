//! Bounded, metadata-only subscriptions. Subscription identity and authority are
//! always derived by Core; guests cannot name a Host, SSH session or credential.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{PluginExtensionTargetId, PluginTerminalState, WireSequence};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginSubscriptionTopic {
    TerminalContext { context_handle: String },
    HostScope {},
    PluginSettings {},
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginSubscriptionEvent {
    TerminalContextChanged {
        context_handle: String,
        target_id: PluginExtensionTargetId,
        generation: WireSequence,
        state: PluginTerminalState,
    },
    TerminalContextEnded {
        context_handle: String,
        generation: WireSequence,
    },
    HostScopeChanged {
        scope_state_version: Option<WireSequence>,
    },
    PluginSettingsChanged {
        revision: WireSequence,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscriptions_are_typed_and_reject_ambient_host_data() {
        let parsed = serde_json::from_str::<PluginSubscriptionTopic>(
            r#"{"kind":"terminalContext","contextHandle":"opaque"}"#,
        );
        let parsed = parsed.expect("typed topic");
        assert!(matches!(
            parsed,
            PluginSubscriptionTopic::TerminalContext { context_handle } if context_handle == "opaque"
        ));
        assert!(
            serde_json::from_str::<PluginSubscriptionTopic>(
                r#"{"kind":"terminalContext","hostId":"host"}"#,
            )
            .is_err()
        );
    }
}
