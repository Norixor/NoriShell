use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The only remote operating system supported by this command catalog.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteOperatingSystem {
    Linux,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationClass {
    ReadOnly,
    Mutation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalRequirement {
    None,
    /// The Core must obtain a new protected approval for this exact plan and execution attempt.
    CoreProtectedPerExecution,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationKind {
    Text,
    CrontabSnapshot,
    ProcessSnapshot,
    CpuUsage,
}

pub const CPU_USAGE_SAMPLE_DURATION_MS: u32 = 200;
pub const CPU_USAGE_MAX_OUTPUT_BYTES: usize = 4 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperationPrecondition {
    ProcessIdentity(ProcessIdentity),
    CrontabSha256(String),
    NginxConfigTest,
}

/// A command description for the Core SSH exec broker.
///
/// `command` is deliberately data rather than an executable callback. Constructing a plan has no
/// remote side effect. Commands containing replacement crontab content must be excluded from logs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationPlan {
    pub command: String,
    /// Bytes written to the SSH exec channel before EOF. The command itself stays log-safe.
    pub stdin: Option<Vec<u8>>,
    pub target_os: RemoteOperatingSystem,
    pub class: OperationClass,
    pub approval: ApprovalRequirement,
    pub timeout: Duration,
    pub max_output_bytes: usize,
    pub kind: OperationKind,
    /// Whether nonzero stderr may contain markers emitted by this fixed planner.
    pub recognizes_internal_markers: bool,
    pub preconditions: Vec<OperationPrecondition>,
    /// The Core may show this in a protected review but must not emit it to logs or audit text.
    pub command_is_sensitive: bool,
    pub stdin_is_sensitive: bool,
    pub review: OperationReview,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationReview {
    /// Stable operation name for the Core-owned localized review surface.
    pub action: &'static str,
    /// A bounded, non-secret target such as a container, unit, PID, host, or path.
    pub target: Option<String>,
    /// A bounded, non-secret detail such as a signal or replacement content hash.
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    content = "operation",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum RemoteOperation {
    Docker(DockerOperation),
    Systemd(SystemdOperation),
    Disk(DiskOperation),
    Network(NetworkOperation),
    Process(ProcessOperation),
    Crontab(CrontabOperation),
    Nginx(NginxOperation),
    Custom(CustomOperation),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum DockerOperation {
    List {
        #[serde(default)]
        all: bool,
    },
    Stats,
    Logs {
        container_id: String,
        lines: u32,
        since_seconds: Option<u32>,
    },
    Start {
        container_id: String,
    },
    Stop {
        container_id: String,
        timeout_seconds: u16,
    },
    Restart {
        container_id: String,
        timeout_seconds: u16,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SystemdOperation {
    List,
    Status { unit: String },
    Journal { unit: String, lines: u32 },
    Start { unit: String },
    Stop { unit: String },
    Restart { unit: String },
    Enable { unit: String },
    Disable { unit: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum DiskOperation {
    Filesystems,
    Usage {
        path: String,
        max_depth: u8,
    },
    LargeFiles {
        path: String,
        minimum_bytes: u64,
        limit: u16,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum NetworkOperation {
    Dns {
        host: String,
    },
    Tcp {
        host: String,
        port: u16,
        timeout_seconds: u8,
    },
    Http {
        url: String,
        timeout_seconds: u8,
    },
    Routes,
    RouteTo {
        destination: String,
    },
    Tls {
        host: String,
        port: u16,
        server_name: Option<String>,
        timeout_seconds: u8,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ProcessOperation {
    List {
        #[serde(default)]
        sort: ProcessSort,
    },
    Sockets,
    Detail {
        pid: u32,
    },
    CpuUsage {},
    Signal {
        identity: ProcessIdentity,
        signal: ProcessSignal,
    },
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessSort {
    #[default]
    Cpu,
    Memory,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CrontabOperation {
    Read,
    Replace {
        expected_sha256: String,
        content: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum NginxOperation {
    Sites,
    ConfigTest,
    Reload,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CustomOperation {
    /// A complete shell command shown verbatim in the Core-owned approval surface.
    Shell {
        command: String,
        timeout_seconds: u16,
        max_output_bytes: u32,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessIdentity {
    pub pid: u32,
    /// Linux `/proc/<pid>/stat` field 22, measured in clock ticks since boot.
    pub start_time_ticks: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessSignal {
    Term,
    Kill,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanError {
    InvalidParameter { field: &'static str },
    ParameterTooLarge { field: &'static str, maximum: usize },
    KnownSecretMaterial,
    CommandTooLarge,
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidParameter { field } => {
                write!(formatter, "invalid operation parameter: {field}")
            }
            Self::ParameterTooLarge { field, maximum } => {
                write!(
                    formatter,
                    "operation parameter {field} exceeds {maximum} bytes"
                )
            }
            Self::KnownSecretMaterial => {
                formatter.write_str("the command contains known secret material")
            }
            Self::CommandTooLarge => {
                formatter.write_str("the planned command exceeds its byte limit")
            }
        }
    }
}

impl std::error::Error for PlanError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawOperationResult {
    pub exit_status: Option<u32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
    pub output_limit_exceeded: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParsedOperationOutput {
    Text(Vec<u8>),
    CrontabSnapshot {
        sha256: String,
        content: Vec<u8>,
    },
    ProcessSnapshot {
        identity: ProcessIdentity,
        details: Vec<u8>,
    },
    CpuUsage {
        basis_points: u16,
        sample_duration_ms: u32,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationSuccess {
    pub output: ParsedOperationOutput,
    pub stderr: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PreconditionFailure {
    ProcessIdentityChanged,
    CrontabChanged,
    NginxConfigTestFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperationFailureKind {
    MissingTool { tool: String },
    PreconditionFailed(PreconditionFailure),
    TimedOut,
    OutputLimitExceeded,
    ConnectionClosed,
    NonZeroExit { exit_status: u32 },
    InvalidStructuredOutput,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationFailure {
    pub kind: OperationFailureKind,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperationOutcome {
    Succeeded(OperationSuccess),
    Failed(OperationFailure),
}
