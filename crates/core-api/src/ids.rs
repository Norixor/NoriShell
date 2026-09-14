use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use ts_rs::TS;
use uuid::{Uuid, Version};

/// UUID-backed request identifier. Request IDs may use any RFC 9562 UUID version.
#[derive(Debug, Clone, PartialEq, Eq, Hash, TS)]
#[ts(type = "string")]
pub struct RequestId(String);

impl RequestId {
    pub fn parse(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        Uuid::parse_str(&value).map_err(|_| "request_id must be a UUID")?;
        Ok(Self(value.to_ascii_lowercase()))
    }

    #[must_use]
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for RequestId {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for RequestId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RequestId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(de::Error::custom)
    }
}

macro_rules! uuid_id {
    ($name:ident, $error:literal, $requires_v7:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, TS)]
        #[ts(type = "string")]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: impl Into<String>) -> Result<Self, &'static str> {
                let value = value.into();
                let parsed = Uuid::parse_str(&value).map_err(|_| $error)?;
                if $requires_v7 && parsed.get_version() != Some(Version::SortRand) {
                    return Err($error);
                }
                Ok(Self(parsed.to_string()))
            }

            #[must_use]
            #[cfg(not(target_arch = "wasm32"))]
            pub fn new() -> Self {
                Self(Uuid::now_v7().to_string())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = &'static str;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::parse(value)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(value).map_err(de::Error::custom)
            }
        }
    };
}

uuid_id!(OperationId, "operation_id must be a UUIDv7", true);
uuid_id!(
    PluginOperationId,
    "plugin_operation_id must be a UUIDv7",
    true
);
uuid_id!(
    PluginInputApprovalId,
    "plugin_input_approval_id must be a UUIDv7",
    true
);
uuid_id!(
    PluginObserverId,
    "plugin_observer_id must be a UUIDv7",
    true
);
uuid_id!(
    PluginApprovalId,
    "plugin_approval_id must be a UUIDv7",
    true
);
uuid_id!(HostId, "host_id must be a UUIDv7", true);
uuid_id!(IdentityId, "identity_id must be a UUIDv7", true);
uuid_id!(CredentialRefId, "credential_ref_id must be a UUIDv7", true);
uuid_id!(KnownHostId, "known_host_id must be a UUIDv7", true);
uuid_id!(SshSessionId, "ssh_session_id must be a UUIDv7", true);
uuid_id!(
    MetricsSessionId,
    "metrics_session_id must be a UUIDv7",
    true
);
uuid_id!(
    MetricsHostKeyChallengeId,
    "metrics_host_key_challenge_id must be a UUIDv7",
    true
);
uuid_id!(
    MetricsKeyboardInteractiveChallengeId,
    "metrics_keyboard_interactive_challenge_id must be a UUIDv7",
    true
);
uuid_id!(
    MetricsKeyboardInteractiveAnswerRefId,
    "metrics_keyboard_interactive_answer_ref_id must be a UUIDv7",
    true
);
uuid_id!(SshAttachmentId, "ssh_attachment_id must be a UUIDv7", true);
uuid_id!(
    SshHostKeyChallengeId,
    "ssh_host_key_challenge_id must be a UUIDv7",
    true
);
uuid_id!(
    SshKeyboardInteractiveChallengeId,
    "ssh_keyboard_interactive_challenge_id must be a UUIDv7",
    true
);
uuid_id!(
    SshKeyboardInteractiveAnswerRefId,
    "ssh_keyboard_interactive_answer_ref_id must be a UUIDv7",
    true
);
uuid_id!(SshInputLeaseId, "ssh_input_lease_id must be a UUIDv7", true);
uuid_id!(SshChannelId, "ssh_channel_id must be a UUIDv7", true);
uuid_id!(SshViewId, "ssh_view_id must be a UUIDv7", true);
uuid_id!(
    SshOpenAttemptId,
    "ssh_open_attempt_id must be a UUIDv7",
    true
);
uuid_id!(
    SshAttachAttemptId,
    "ssh_attach_attempt_id must be a UUIDv7",
    true
);
uuid_id!(LocalSessionId, "local_session_id must be a UUIDv7", true);
uuid_id!(
    LocalAttachmentId,
    "local_attachment_id must be a UUIDv7",
    true
);
uuid_id!(
    LocalInputLeaseId,
    "local_input_lease_id must be a UUIDv7",
    true
);
uuid_id!(LocalPtyId, "local_pty_id must be a UUIDv7", true);
uuid_id!(LocalViewId, "local_view_id must be a UUIDv7", true);
uuid_id!(
    LocalOpenAttemptId,
    "local_open_attempt_id must be a UUIDv7",
    true
);
uuid_id!(
    LocalAttachAttemptId,
    "local_attach_attempt_id must be a UUIDv7",
    true
);
uuid_id!(TelnetSessionId, "telnet_session_id must be a UUIDv7", true);
uuid_id!(
    TelnetAttachmentId,
    "telnet_attachment_id must be a UUIDv7",
    true
);
uuid_id!(
    TelnetInputLeaseId,
    "telnet_input_lease_id must be a UUIDv7",
    true
);
uuid_id!(TelnetSocketId, "telnet_socket_id must be a UUIDv7", true);
uuid_id!(TelnetViewId, "telnet_view_id must be a UUIDv7", true);
uuid_id!(
    TelnetOpenAttemptId,
    "telnet_open_attempt_id must be a UUIDv7",
    true
);
uuid_id!(
    TelnetAttachAttemptId,
    "telnet_attach_attempt_id must be a UUIDv7",
    true
);
uuid_id!(SftpSessionId, "sftp_session_id must be a UUIDv7", true);
uuid_id!(TransferId, "transfer_id must be a UUIDv7", true);
uuid_id!(ForwardRuleId, "forward_rule_id must be a UUIDv7", true);
uuid_id!(
    ForwardSessionId,
    "forward_session_id must be a UUIDv7",
    true
);
uuid_id!(
    NativeTerminalEventId,
    "native_terminal_event_id must be a UUIDv7",
    true
);
uuid_id!(
    NativeTerminalHistoryEntryId,
    "native_terminal_history_entry_id must be a UUIDv7",
    true
);
uuid_id!(SecretRefId, "secret_ref_id must be a UUID", false);

/// Public short-lived handle for one staged login-automation secret. The same
/// wrapper is also used internally for legacy persisted references so the
/// public wire contract never names or exposes `SecretRefId`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, TS)]
#[ts(type = "string")]
pub struct LoginAutomationSecretStageId(SecretRefId);

impl LoginAutomationSecretStageId {
    pub fn parse(value: impl Into<String>) -> Result<Self, &'static str> {
        SecretRefId::parse(value).map(Self)
    }

    #[must_use]
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new() -> Self {
        Self(SecretRefId::new())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for LoginAutomationSecretStageId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::ops::Deref for LoginAutomationSecretStageId {
    type Target = SecretRefId;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Serialize for LoginAutomationSecretStageId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for LoginAutomationSecretStageId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(de::Error::custom)
    }
}

/// Public short-lived handle for a password staged specifically for one configured Host create
/// operation. It never exposes the internal Vault SecretRef type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, TS)]
#[ts(type = "string")]
pub struct HostCreatePasswordStageId(SecretRefId);

impl HostCreatePasswordStageId {
    pub fn parse(value: impl Into<String>) -> Result<Self, &'static str> {
        SecretRefId::parse(value).map(Self)
    }

    #[must_use]
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new() -> Self {
        Self(SecretRefId::new())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for HostCreatePasswordStageId {
    fn default() -> Self {
        Self::new()
    }
}

impl Serialize for HostCreatePasswordStageId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for HostCreatePasswordStageId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(de::Error::custom)
    }
}

/// Sequence values are decimal strings on the wire to avoid JavaScript number loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, TS)]
#[ts(type = "string")]
pub struct WireSequence(#[ts(type = "string")] pub u64);

impl WireSequence {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl Serialize for WireSequence {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for WireSequence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct WireSequenceVisitor;

        impl de::Visitor<'_> for WireSequenceVisitor {
            type Value = WireSequence;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a decimal u64 encoded as a JSON string")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                value
                    .parse::<u64>()
                    .map(WireSequence)
                    .map_err(|_| E::custom("wire sequence must be a decimal u64 string"))
            }
        }

        deserializer.deserialize_str(WireSequenceVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CredentialRefId, ForwardRuleId, ForwardSessionId, HostId, IdentityId, KnownHostId,
        LocalAttachAttemptId, LocalAttachmentId, LocalInputLeaseId, LocalOpenAttemptId, LocalPtyId,
        LocalSessionId, LocalViewId, MetricsHostKeyChallengeId,
        MetricsKeyboardInteractiveAnswerRefId, MetricsKeyboardInteractiveChallengeId,
        MetricsSessionId, OperationId, SecretRefId, SftpSessionId, SshAttachAttemptId,
        SshAttachmentId, SshChannelId, SshHostKeyChallengeId, SshInputLeaseId, SshOpenAttemptId,
        SshSessionId, SshViewId, TelnetAttachAttemptId, TelnetAttachmentId, TelnetInputLeaseId,
        TelnetOpenAttemptId, TelnetSessionId, TelnetSocketId, TelnetViewId, TransferId,
        WireSequence,
    };

    #[test]
    fn wire_sequence_uses_a_json_string() {
        let encoded = serde_json::to_string(&WireSequence::new(u64::MAX)).expect("serialize");
        assert_eq!(encoded, format!("\"{}\"", u64::MAX));
        let decoded: WireSequence = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded.get(), u64::MAX);
    }

    #[test]
    fn request_id_rejects_non_uuid_input() {
        let decoded = serde_json::from_str::<super::RequestId>("\"not-a-uuid\"");
        assert!(decoded.is_err());
    }

    #[test]
    fn operation_and_resource_ids_require_uuid_v7() {
        let uuid_v4 = uuid::Uuid::new_v4().to_string();
        assert!(OperationId::parse(&uuid_v4).is_err());
        assert!(HostId::parse(&uuid_v4).is_err());
        assert!(MetricsSessionId::parse(&uuid_v4).is_err());
        assert!(MetricsHostKeyChallengeId::parse(&uuid_v4).is_err());
        assert!(MetricsKeyboardInteractiveChallengeId::parse(&uuid_v4).is_err());
        assert!(MetricsKeyboardInteractiveAnswerRefId::parse(&uuid_v4).is_err());
        assert!(IdentityId::parse(&uuid_v4).is_err());
        assert!(CredentialRefId::parse(&uuid_v4).is_err());
        assert!(KnownHostId::parse(&uuid_v4).is_err());
        assert!(SshSessionId::parse(&uuid_v4).is_err());
        assert!(SshAttachmentId::parse(&uuid_v4).is_err());
        assert!(SshHostKeyChallengeId::parse(&uuid_v4).is_err());
        assert!(SshInputLeaseId::parse(&uuid_v4).is_err());
        assert!(SshChannelId::parse(&uuid_v4).is_err());
        assert!(SshViewId::parse(&uuid_v4).is_err());
        assert!(SshOpenAttemptId::parse(&uuid_v4).is_err());
        assert!(SshAttachAttemptId::parse(&uuid_v4).is_err());
        assert!(LocalSessionId::parse(&uuid_v4).is_err());
        assert!(LocalAttachmentId::parse(&uuid_v4).is_err());
        assert!(LocalInputLeaseId::parse(&uuid_v4).is_err());
        assert!(LocalPtyId::parse(&uuid_v4).is_err());
        assert!(LocalViewId::parse(&uuid_v4).is_err());
        assert!(LocalOpenAttemptId::parse(&uuid_v4).is_err());
        assert!(LocalAttachAttemptId::parse(&uuid_v4).is_err());
        assert!(TelnetSessionId::parse(&uuid_v4).is_err());
        assert!(TelnetAttachmentId::parse(&uuid_v4).is_err());
        assert!(TelnetInputLeaseId::parse(&uuid_v4).is_err());
        assert!(TelnetSocketId::parse(&uuid_v4).is_err());
        assert!(TelnetViewId::parse(&uuid_v4).is_err());
        assert!(TelnetOpenAttemptId::parse(&uuid_v4).is_err());
        assert!(TelnetAttachAttemptId::parse(&uuid_v4).is_err());
        assert!(SftpSessionId::parse(&uuid_v4).is_err());
        assert!(TransferId::parse(&uuid_v4).is_err());
        assert!(ForwardRuleId::parse(&uuid_v4).is_err());
        assert!(ForwardSessionId::parse(&uuid_v4).is_err());
        assert!(SecretRefId::parse(&uuid_v4).is_ok());
    }
}
