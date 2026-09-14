use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const CORE_API_MAJOR: u16 = 1;
pub const CORE_API_MINOR: u16 = 85;
pub const PLUGIN_PROTOCOL_MAJOR: u16 = 1;
pub const PLUGIN_TEMPLATE_ON_OPEN_PROTOCOL_MINOR: u16 = 11;
pub const PLUGIN_SETTINGS_PROTOCOL_MINOR: u16 = 12;
pub const PLUGIN_API_PROTOCOL_MINOR: u16 = 13;
/// Pure data theme packages use a newer manifest minor without changing the
/// resident Wasm guest ABI. Wasm packages remain pinned to
/// [`PLUGIN_PROTOCOL_MINOR`].
pub const PLUGIN_THEME_PROTOCOL_MINOR: u16 = 14;
pub const PLUGIN_PROTOCOL_MINOR: u16 = PLUGIN_API_PROTOCOL_MINOR;
pub const PLUGIN_PROTOCOL_MAX_MINOR: u16 = PLUGIN_THEME_PROTOCOL_MINOR;

/// Minor 13 is the first stable resident Wasm ABI. Earlier ABIs are not runnable.
pub const PLUGIN_PROTOCOL_MIN_MINOR: u16 = 13;

/// Future minor releases must retain the wire contract for every accepted minor.
/// New fields/events must be projected for the guest's manifest minor at dispatch.
#[must_use]
pub const fn plugin_protocol_is_compatible(major: u16, minor: u16) -> bool {
    plugin_protocol_is_supported_by(PLUGIN_PROTOCOL_MAJOR, PLUGIN_PROTOCOL_MINOR, major, minor)
}

/// Package inspection accepts the theme-only manifest minor in addition to
/// the resident Wasm ABI window. Callers that launch a guest must use
/// [`plugin_protocol_is_compatible`] instead.
#[must_use]
pub const fn plugin_package_protocol_is_compatible(major: u16, minor: u16) -> bool {
    plugin_protocol_is_supported_by(
        PLUGIN_PROTOCOL_MAJOR,
        PLUGIN_PROTOCOL_MAX_MINOR,
        major,
        minor,
    )
}

#[must_use]
pub const fn plugin_protocol_is_supported_by(
    host_major: u16,
    host_minor: u16,
    major: u16,
    minor: u16,
) -> bool {
    major == host_major && minor >= PLUGIN_PROTOCOL_MIN_MINOR && minor <= host_minor
}

/// Exhaustive introduction table: adding a capability requires an explicit guest-version decision.
#[must_use]
pub const fn plugin_capability_min_protocol_minor(capability: crate::PluginCapability) -> u16 {
    use crate::PluginCapability::*;
    match capability {
        UiPanel | UiNavigation | UiPage | UiWebviewIsolated | UiHostDomObserve
        | UiHostDomMutate | UiHostCss | ClipboardWrite | TerminalProvider | DeviceSerial
        | TerminalMetadata | TerminalObserve | TerminalAnnotation | TerminalProposeInput
        | TerminalRequestInput | HostMetadataRead | HostMutationPropose | HostSessionRequest
        | RemoteInspect | RemoteExecRequest | NetworkDomain | LocalFiles | LocalProcess
        | StoragePlugin | CredentialsPlugin | SftpRead | SftpWrite | MetricsRead | SshSync => {
            PLUGIN_PROTOCOL_MIN_MINOR
        }
    }
}

/// New event kinds must declare their first guest minor; old guests never receive them.
#[must_use]
pub const fn plugin_message_min_protocol_minor(kind: crate::PluginHostMessageKind) -> u16 {
    use crate::PluginHostMessageKind::*;
    match kind {
        Initialize | Invoke | UiAction | SshSyncResult | BrokerResult | TerminalObservation
        | ProtocolEvent | WorkflowEvent => PLUGIN_PROTOCOL_MIN_MINOR,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CoreApiVersion {
    pub major: u16,
    pub minor: u16,
}

impl CoreApiVersion {
    #[must_use]
    pub const fn current() -> Self {
        Self {
            major: CORE_API_MAJOR,
            minor: CORE_API_MINOR,
        }
    }

    #[must_use]
    pub const fn is_major_compatible(self, other: Self) -> bool {
        self.major == other.major
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CORE_API_MAJOR, CORE_API_MINOR, CoreApiVersion, PLUGIN_API_PROTOCOL_MINOR,
        PLUGIN_PROTOCOL_MAX_MINOR, PLUGIN_PROTOCOL_MINOR, PLUGIN_SETTINGS_PROTOCOL_MINOR,
        PLUGIN_TEMPLATE_ON_OPEN_PROTOCOL_MINOR, PLUGIN_THEME_PROTOCOL_MINOR,
    };

    #[test]
    fn stable_abi_window_is_backward_compatible_and_bounded() {
        for host in [13, 14, 20] {
            for guest in 0..=21 {
                assert_eq!(
                    super::plugin_protocol_is_supported_by(1, host, 1, guest),
                    (13..=host).contains(&guest)
                );
                assert!(!super::plugin_protocol_is_supported_by(1, host, 2, guest));
            }
        }
        assert!(super::plugin_protocol_is_supported_by(1, 14, 1, 13));
        assert!(!super::plugin_protocol_is_supported_by(1, 12, 1, 12));
    }

    #[test]
    fn current_version_exposes_install_permission_target_minor() {
        assert_eq!(CORE_API_MAJOR, 1);
        assert_eq!(CORE_API_MINOR, 85);
        assert_eq!(CoreApiVersion::current().minor, 85);
        assert_eq!(PLUGIN_TEMPLATE_ON_OPEN_PROTOCOL_MINOR, 11);
        assert_eq!(PLUGIN_SETTINGS_PROTOCOL_MINOR, 12);
        assert_eq!(PLUGIN_API_PROTOCOL_MINOR, 13);
        assert_eq!(PLUGIN_PROTOCOL_MINOR, PLUGIN_API_PROTOCOL_MINOR);
        assert_eq!(PLUGIN_THEME_PROTOCOL_MINOR, 14);
        assert_eq!(PLUGIN_PROTOCOL_MAX_MINOR, PLUGIN_THEME_PROTOCOL_MINOR);
        assert!(!super::plugin_protocol_is_compatible(
            1,
            PLUGIN_THEME_PROTOCOL_MINOR
        ));
        assert!(super::plugin_package_protocol_is_compatible(
            1,
            PLUGIN_THEME_PROTOCOL_MINOR
        ));
    }
}
