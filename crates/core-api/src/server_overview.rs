use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use ts_rs::TS;

use crate::{
    CredentialRefId, DiskResourceId, HostCatalogEntry, HostId, MetricsHostKeyChallengeId,
    MetricsKeyboardInteractiveAnswerRefId, MetricsKeyboardInteractiveChallengeId, MetricsSessionId,
    MonitoringPolicySummary, NetworkResourceId, OperationId, RequestMeta, SshSessionId,
    SshSessionRouteStage, WireSequence,
};

/// An unsigned byte count encoded as a decimal string so JavaScript cannot
/// silently round large filesystem or network counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, TS)]
#[ts(type = "string")]
pub struct MetricByteCount(#[ts(type = "string")] pub u64);

impl MetricByteCount {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl Serialize for MetricByteCount {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for MetricByteCount {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor;

        impl de::Visitor<'_> for Visitor {
            type Value = MetricByteCount;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an unsigned decimal byte count encoded as a string")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                value
                    .parse::<u64>()
                    .map(MetricByteCount)
                    .map_err(|_| E::custom("metric byte count must be an unsigned decimal string"))
            }
        }

        deserializer.deserialize_str(Visitor)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum MetricsSessionState {
    Idle,
    Connecting,
    VerifyingHostKey,
    NeedsHostKeyReview,
    Authenticating,
    NeedsAuthentication,
    DetectingPlatform,
    Sampling,
    Ready,
    Backoff,
    Disconnecting,
    Closed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum MetricsAuthenticationReason {
    VaultLocked,
    KeyboardInteractive,
    CredentialUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum MetricsPlatform {
    Linux,
    Macos,
    Windows,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum MetricFieldState {
    Available,
    InitialBaseline,
    CounterReset,
    CounterSetChanged,
    NoCounterProgress,
    Unsupported,
    PermissionDenied,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CpuMetric {
    pub state: MetricFieldState,
    /// Hundredths of one percent, so 10_000 represents 100%.
    pub basis_points: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MemoryMetric {
    pub state: MetricFieldState,
    pub used_bytes: Option<MetricByteCount>,
    pub available_bytes: Option<MetricByteCount>,
    pub total_bytes: Option<MetricByteCount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NetworkMetric {
    pub resource_id: NetworkResourceId,
    pub state: MetricFieldState,
    pub receive_bytes_per_second: Option<MetricByteCount>,
    pub transmit_bytes_per_second: Option<MetricByteCount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DiskMetric {
    pub resource_id: DiskResourceId,
    pub state: MetricFieldState,
    /// Provider-owned identifier. It is display data and never a shell fragment.
    pub filesystem_id: String,
    /// Provider-owned verified mount label. It is never interpolated into a command.
    pub mount: String,
    pub used_bytes: Option<MetricByteCount>,
    pub available_bytes: Option<MetricByteCount>,
    pub total_bytes: Option<MetricByteCount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MetricSnapshot {
    pub host_id: HostId,
    pub metrics_session_id: MetricsSessionId,
    pub generation: WireSequence,
    pub sample_sequence: WireSequence,
    pub provider_id: String,
    pub provider_version: u16,
    pub platform: MetricsPlatform,
    #[ts(type = "number")]
    pub sample_started_at_unix_ms: i64,
    #[ts(type = "number")]
    pub sample_completed_at_unix_ms: i64,
    pub stale: bool,
    pub cpu: CpuMetric,
    pub memory: MemoryMetric,
    pub network: NetworkMetric,
    pub disks: Vec<DiskMetric>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum MetricsSessionFailureCode {
    ConnectionUnavailable,
    HostKeyMismatch,
    AuthenticationRejected,
    ProviderUnsupported,
    PermissionDenied,
    SampleTimedOut,
    OutputLimitExceeded,
    MalformedOutput,
    TransportLost,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSessionSummary {
    pub metrics_session_id: MetricsSessionId,
    pub host_id: HostId,
    pub generation: WireSequence,
    pub state_revision: WireSequence,
    pub state: MetricsSessionState,
    pub authentication_reason: Option<MetricsAuthenticationReason>,
    pub failure_code: Option<MetricsSessionFailureCode>,
    #[ts(type = "number | null")]
    pub next_retry_at_unix_ms: Option<i64>,
    pub latest_snapshot: Option<MetricSnapshot>,
    pub host_key_challenge: Option<MetricsHostKeyChallenge>,
    pub keyboard_interactive_challenge: Option<MetricsKeyboardInteractiveChallenge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MetricsHostKeyChallenge {
    pub challenge_id: MetricsHostKeyChallengeId,
    pub metrics_session_id: MetricsSessionId,
    pub host_id: HostId,
    pub generation: WireSequence,
    pub route_stage: SshSessionRouteStage,
    pub algorithm: String,
    pub fingerprint_sha256: String,
    pub trusted_fingerprint_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum MetricsHostKeyDecision {
    Accept,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MetricsKeyboardInteractivePrompt {
    pub prompt_index: u8,
    pub label: String,
    pub echo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MetricsKeyboardInteractiveChallenge {
    pub challenge_id: MetricsKeyboardInteractiveChallengeId,
    pub metrics_session_id: MetricsSessionId,
    pub host_id: HostId,
    pub generation: WireSequence,
    pub route_stage: SshSessionRouteStage,
    pub credential_ref_id: CredentialRefId,
    pub attempt_index: u8,
    pub round_index: u8,
    pub name: String,
    pub instruction: String,
    pub prompts: Vec<MetricsKeyboardInteractivePrompt>,
    #[ts(type = "number")]
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MetricsReconcileRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MetricsRetryRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub host_id: HostId,
    pub expected_generation: Option<WireSequence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MetricsStopRequest {
    pub meta: RequestMeta,
    pub host_id: HostId,
    pub expected_generation: Option<WireSequence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MetricsHostKeyDecisionRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub metrics_session_id: MetricsSessionId,
    pub host_id: HostId,
    pub expected_generation: WireSequence,
    pub challenge_id: MetricsHostKeyChallengeId,
    pub decision: MetricsHostKeyDecision,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MetricsKeyboardInteractiveAnswerPrepareRequest {
    pub meta: RequestMeta,
    pub answer_ref_id: MetricsKeyboardInteractiveAnswerRefId,
    pub metrics_session_id: MetricsSessionId,
    pub expected_generation: WireSequence,
    pub challenge_id: MetricsKeyboardInteractiveChallengeId,
    pub round_index: u8,
    pub prompt_index: u8,
    pub value: String,
}

impl fmt::Debug for MetricsKeyboardInteractiveAnswerPrepareRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MetricsKeyboardInteractiveAnswerPrepareRequest")
            .field("meta", &self.meta)
            .field("answer_ref_id", &self.answer_ref_id)
            .field("metrics_session_id", &self.metrics_session_id)
            .field("expected_generation", &self.expected_generation)
            .field("challenge_id", &self.challenge_id)
            .field("round_index", &self.round_index)
            .field("prompt_index", &self.prompt_index)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MetricsKeyboardInteractiveAnswerPrepareResponse {
    pub answer_ref_id: MetricsKeyboardInteractiveAnswerRefId,
    #[ts(type = "number")]
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MetricsKeyboardInteractiveRespondRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub metrics_session_id: MetricsSessionId,
    pub host_id: HostId,
    pub expected_generation: WireSequence,
    pub challenge_id: MetricsKeyboardInteractiveChallengeId,
    pub round_index: u8,
    pub answer_ref_ids: Vec<MetricsKeyboardInteractiveAnswerRefId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ServerOverviewConnectionState {
    Connecting,
    Connected,
    MonitoringOnly,
    Degraded,
    Disconnected,
    Failed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ServerResourceCounts {
    pub connecting: u32,
    pub running: u32,
    pub lost: u32,
    pub failed: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ServerOverviewCard {
    pub catalog_entry: HostCatalogEntry,
    pub monitoring_policy: MonitoringPolicySummary,
    pub connection_state: ServerOverviewConnectionState,
    pub terminal_counts: ServerResourceCounts,
    pub sftp_counts: ServerResourceCounts,
    pub forward_counts: ServerResourceCounts,
    pub metrics_counts: ServerResourceCounts,
    pub terminal_session_ids: Vec<SshSessionId>,
    pub metrics_session: Option<MetricsSessionSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ServerOverviewSnapshotRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ServerOverviewSnapshot {
    pub snapshot_revision: WireSequence,
    pub cards: Vec<ServerOverviewCard>,
}

#[cfg(test)]
mod tests {
    use crate::{
        MetricsKeyboardInteractiveAnswerRefId, MetricsKeyboardInteractiveChallengeId,
        MetricsSessionId, RequestId, RequestMeta, WireSequence,
    };

    use super::{
        MetricByteCount, MetricFieldState, MetricsKeyboardInteractiveAnswerPrepareRequest,
        NetworkMetric,
    };

    #[test]
    fn byte_counts_remain_exact_on_the_wire() {
        let value = MetricByteCount::new(u64::MAX);
        let encoded = serde_json::to_string(&value).expect("serialize byte count");
        assert_eq!(encoded, format!("\"{}\"", u64::MAX));
        let decoded: MetricByteCount = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded.get(), u64::MAX);
    }

    #[test]
    fn zero_network_rate_is_not_an_unavailable_value() {
        let metric = NetworkMetric {
            resource_id: crate::NetworkResourceId::AggregateNonLoopback,
            state: MetricFieldState::Available,
            receive_bytes_per_second: Some(MetricByteCount::new(0)),
            transmit_bytes_per_second: Some(MetricByteCount::new(0)),
        };
        let encoded = serde_json::to_value(metric).expect("serialize network metric");
        assert_eq!(encoded["state"], "available");
        assert_eq!(encoded["resourceId"], "aggregateNonLoopback");
        assert_eq!(encoded["receiveBytesPerSecond"], "0");
        assert_eq!(encoded["transmitBytesPerSecond"], "0");
    }

    #[test]
    fn keyboard_interactive_answer_debug_redacts_the_secret_value() {
        let request = MetricsKeyboardInteractiveAnswerPrepareRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            answer_ref_id: MetricsKeyboardInteractiveAnswerRefId::new(),
            metrics_session_id: MetricsSessionId::new(),
            expected_generation: WireSequence::new(1),
            challenge_id: MetricsKeyboardInteractiveChallengeId::new(),
            round_index: 1,
            prompt_index: 0,
            value: "do-not-log-this-answer".to_owned(),
        };
        let rendered = format!("{request:?}");
        assert!(!rendered.contains("do-not-log-this-answer"));
        assert!(rendered.contains("[REDACTED]"));
    }
}
