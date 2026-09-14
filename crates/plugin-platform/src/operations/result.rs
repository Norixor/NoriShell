use super::*;

const CRONTAB_HEADER: &[u8] = b"NORISHELL_CRONTAB_SHA256:";
const PROCESS_HEADER: &[u8] = b"NORISHELL_PROCESS_IDENTITY:";
const CPU_FIRST_HEADER: &[u8] = b"NORISHELL_CPU_STAT:first:";
const CPU_SECOND_HEADER: &[u8] = b"NORISHELL_CPU_STAT:second:";
const ERROR_PREFIX: &str = "NORISHELL_OP_ERROR:";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResultParseError {
    CapturedOutputExceedsPlan,
}

impl std::fmt::Display for ResultParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CapturedOutputExceedsPlan => {
                formatter.write_str("captured remote output exceeds the operation plan")
            }
        }
    }
}

impl std::error::Error for ResultParseError {}

pub fn parse_result(
    plan: &OperationPlan,
    raw: RawOperationResult,
) -> Result<OperationOutcome, ResultParseError> {
    if raw.output_limit_exceeded {
        return Ok(failure(OperationFailureKind::OutputLimitExceeded, raw));
    }
    if raw.stdout.len().saturating_add(raw.stderr.len()) > plan.max_output_bytes {
        return Err(ResultParseError::CapturedOutputExceedsPlan);
    }
    if raw.timed_out {
        return Ok(failure(OperationFailureKind::TimedOut, raw));
    }
    let Some(exit_status) = raw.exit_status else {
        return Ok(failure(OperationFailureKind::ConnectionClosed, raw));
    };
    if exit_status != 0 {
        let kind = if plan.recognizes_internal_markers {
            classify_failure(exit_status, &raw.stderr)
        } else {
            OperationFailureKind::NonZeroExit { exit_status }
        };
        return Ok(failure(kind, raw));
    }

    let output = match parse_success(plan.kind, &raw.stdout) {
        Some(output) => output,
        None => {
            return Ok(failure(OperationFailureKind::InvalidStructuredOutput, raw));
        }
    };
    Ok(OperationOutcome::Succeeded(OperationSuccess {
        output,
        stderr: raw.stderr,
    }))
}

fn failure(kind: OperationFailureKind, raw: RawOperationResult) -> OperationOutcome {
    OperationOutcome::Failed(OperationFailure {
        kind,
        stdout: raw.stdout,
        stderr: raw.stderr,
    })
}

fn classify_failure(exit_status: u32, stderr: &[u8]) -> OperationFailureKind {
    if let Ok(stderr) = std::str::from_utf8(stderr) {
        for line in stderr.lines() {
            let Some(marker) = line.strip_prefix(ERROR_PREFIX) else {
                continue;
            };
            if exit_status == 127
                && let Some(tool) = marker.strip_prefix("missing-tool:")
                && !tool.is_empty()
                && tool.len() <= 256
                && tool.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-' | b'+' | b'/')
                })
            {
                return OperationFailureKind::MissingTool {
                    tool: tool.to_owned(),
                };
            }
            if exit_status == 73 {
                return match marker {
                    "process-identity-mismatch" => OperationFailureKind::PreconditionFailed(
                        PreconditionFailure::ProcessIdentityChanged,
                    ),
                    "crontab-cas-mismatch" => OperationFailureKind::PreconditionFailed(
                        PreconditionFailure::CrontabChanged,
                    ),
                    _ => OperationFailureKind::NonZeroExit { exit_status },
                };
            }
            if marker == "nginx-config-test-failed" {
                return OperationFailureKind::PreconditionFailed(
                    PreconditionFailure::NginxConfigTestFailed,
                );
            }
        }
    }
    OperationFailureKind::NonZeroExit { exit_status }
}

fn parse_success(kind: OperationKind, stdout: &[u8]) -> Option<ParsedOperationOutput> {
    match kind {
        OperationKind::Text => Some(ParsedOperationOutput::Text(stdout.to_vec())),
        OperationKind::CrontabSnapshot => {
            let (header, content) = split_first_line(stdout)?;
            let hash = header.strip_prefix(CRONTAB_HEADER)?;
            if hash.len() != 64 || !hash.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            Some(ParsedOperationOutput::CrontabSnapshot {
                sha256: std::str::from_utf8(hash).ok()?.to_ascii_lowercase(),
                content: content.to_vec(),
            })
        }
        OperationKind::ProcessSnapshot => {
            let (header, details) = split_first_line(stdout)?;
            let identity = header.strip_prefix(PROCESS_HEADER)?;
            let identity = std::str::from_utf8(identity).ok()?;
            let (pid, start_time_ticks) = identity.split_once(':')?;
            let pid = pid.parse().ok()?;
            let start_time_ticks = start_time_ticks.parse().ok()?;
            if pid < 2 || start_time_ticks == 0 {
                return None;
            }
            Some(ParsedOperationOutput::ProcessSnapshot {
                identity: ProcessIdentity {
                    pid,
                    start_time_ticks,
                },
                details: details.to_vec(),
            })
        }
        OperationKind::CpuUsage => parse_cpu_usage(stdout),
    }
}

fn parse_cpu_usage(stdout: &[u8]) -> Option<ParsedOperationOutput> {
    if stdout.len() > CPU_USAGE_MAX_OUTPUT_BYTES {
        return None;
    }
    let stdout = stdout.strip_suffix(b"\n")?;
    let mut lines = stdout.split(|byte| *byte == b'\n');
    let first = lines.next()?.strip_prefix(CPU_FIRST_HEADER)?;
    let second = lines.next()?.strip_prefix(CPU_SECOND_HEADER)?;
    if lines.next().is_some() {
        return None;
    }
    let first = parse_cpu_counters(first)?;
    let second = parse_cpu_counters(second)?;
    let mut total_delta = 0_u64;
    let mut idle_delta = 0_u64;
    for (index, (first, second)) in first.iter().zip(second.iter()).enumerate() {
        let delta = second.checked_sub(*first)?;
        total_delta = total_delta.checked_add(delta)?;
        if matches!(index, 3 | 4) {
            idle_delta = idle_delta.checked_add(delta)?;
        }
    }
    if total_delta == 0 {
        return None;
    }
    let active_delta = total_delta.checked_sub(idle_delta)?;
    let basis_points =
        u16::try_from(u128::from(active_delta).checked_mul(10_000)? / u128::from(total_delta))
            .ok()?;
    if basis_points > 10_000 {
        return None;
    }
    Some(ParsedOperationOutput::CpuUsage {
        basis_points,
        sample_duration_ms: CPU_USAGE_SAMPLE_DURATION_MS,
    })
}

fn parse_cpu_counters(line: &[u8]) -> Option<[u64; 8]> {
    let line = std::str::from_utf8(line).ok()?;
    if line.bytes().any(|byte| byte.is_ascii_control()) {
        return None;
    }
    let mut fields = line.split_ascii_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    let values = fields
        .map(|field| field.parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()?;
    if !(8..=16).contains(&values.len()) {
        return None;
    }
    values[..8].try_into().ok()
}

fn split_first_line(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let newline = bytes.iter().position(|byte| *byte == b'\n')?;
    Some((&bytes[..newline], &bytes[newline + 1..]))
}

#[cfg(test)]
mod tests {
    use super::super::plan;
    use super::*;

    fn raw(exit_status: Option<u32>, stdout: &[u8], stderr: &[u8]) -> RawOperationResult {
        RawOperationResult {
            exit_status,
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
            timed_out: false,
            output_limit_exceeded: false,
        }
    }

    #[test]
    fn missing_tool_is_explicit() {
        let plan = plan(&RemoteOperation::Docker(DockerOperation::List {
            all: true,
        }))
        .unwrap();
        let result = parse_result(
            &plan,
            raw(Some(127), b"", b"NORISHELL_OP_ERROR:missing-tool:docker\n"),
        )
        .unwrap();
        assert!(matches!(
            result,
            OperationOutcome::Failed(OperationFailure {
                kind: OperationFailureKind::MissingTool { ref tool },
                ..
            }) if tool == "docker"
        ));
    }

    #[test]
    fn nonzero_exit_and_stderr_are_preserved() {
        let plan = plan(&RemoteOperation::Systemd(SystemdOperation::Status {
            unit: "demo.service".to_owned(),
        }))
        .unwrap();
        let result = parse_result(&plan, raw(Some(4), b"partial", b"not found")).unwrap();
        assert_eq!(
            result,
            OperationOutcome::Failed(OperationFailure {
                kind: OperationFailureKind::NonZeroExit { exit_status: 4 },
                stdout: b"partial".to_vec(),
                stderr: b"not found".to_vec(),
            })
        );
    }

    #[test]
    fn custom_output_cannot_spoof_internal_failure_markers() {
        let plan = plan(&RemoteOperation::Custom(CustomOperation::Shell {
            command: "exit 127".to_owned(),
            timeout_seconds: 5,
            max_output_bytes: 4096,
        }))
        .unwrap();
        let result = parse_result(
            &plan,
            raw(Some(127), b"", b"NORISHELL_OP_ERROR:missing-tool:docker\n"),
        )
        .unwrap();
        assert!(matches!(
            result,
            OperationOutcome::Failed(OperationFailure {
                kind: OperationFailureKind::NonZeroExit { exit_status: 127 },
                ..
            })
        ));
    }

    #[test]
    fn crontab_snapshot_extracts_hash_and_exact_content() {
        let plan = plan(&RemoteOperation::Crontab(CrontabOperation::Read)).unwrap();
        let hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let stdout = format!("NORISHELL_CRONTAB_SHA256:{hash}\nMAILTO=a@example.com\n\n");
        let result = parse_result(&plan, raw(Some(0), stdout.as_bytes(), b"")).unwrap();
        assert_eq!(
            result,
            OperationOutcome::Succeeded(OperationSuccess {
                output: ParsedOperationOutput::CrontabSnapshot {
                    sha256: hash.to_owned(),
                    content: b"MAILTO=a@example.com\n\n".to_vec(),
                },
                stderr: Vec::new(),
            })
        );
    }

    #[test]
    fn process_snapshot_returns_identity_for_a_later_signal_precondition() {
        let plan = plan(&RemoteOperation::Process(ProcessOperation::Detail {
            pid: 42,
        }))
        .unwrap();
        let result = parse_result(
            &plan,
            raw(
                Some(0),
                b"NORISHELL_PROCESS_IDENTITY:42:987654\n42 1 1000 user S process\n",
                b"",
            ),
        )
        .unwrap();
        assert!(matches!(
            result,
            OperationOutcome::Succeeded(OperationSuccess {
                output: ParsedOperationOutput::ProcessSnapshot {
                    identity: ProcessIdentity {
                        pid: 42,
                        start_time_ticks: 987654
                    },
                    ..
                },
                ..
            })
        ));
    }

    fn cpu_stdout(first: &str, second: &str) -> Vec<u8> {
        format!("NORISHELL_CPU_STAT:first:{first}\nNORISHELL_CPU_STAT:second:{second}\n")
            .into_bytes()
    }

    fn parse_cpu(first: &str, second: &str) -> OperationOutcome {
        let plan = plan(&RemoteOperation::Process(ProcessOperation::CpuUsage {})).unwrap();
        parse_result(&plan, raw(Some(0), &cpu_stdout(first, second), b"")).unwrap()
    }

    #[test]
    fn cpu_usage_uses_aggregate_counter_deltas_and_preserves_zero_and_full_utilization() {
        let half = parse_cpu("cpu 100 0 100 800 0 0 0 0", "cpu 150 0 150 900 0 0 0 0");
        assert!(matches!(
            half,
            OperationOutcome::Succeeded(OperationSuccess {
                output: ParsedOperationOutput::CpuUsage {
                    basis_points: 5_000,
                    sample_duration_ms: CPU_USAGE_SAMPLE_DURATION_MS,
                },
                ..
            })
        ));
        let zero = parse_cpu("cpu 100 0 100 800 0 0 0 0", "cpu 100 0 100 900 0 0 0 0");
        assert!(matches!(
            zero,
            OperationOutcome::Succeeded(OperationSuccess {
                output: ParsedOperationOutput::CpuUsage {
                    basis_points: 0,
                    ..
                },
                ..
            })
        ));
        let full = parse_cpu("cpu 100 0 100 800 0 0 0 0", "cpu 200 0 100 800 0 0 0 0");
        assert!(matches!(
            full,
            OperationOutcome::Succeeded(OperationSuccess {
                output: ParsedOperationOutput::CpuUsage {
                    basis_points: 10_000,
                    ..
                },
                ..
            })
        ));
    }

    #[test]
    fn cpu_usage_fails_closed_for_resets_zero_deltas_malformed_output_and_guest_columns() {
        for (first, second) in [
            ("cpu 100 0 100 800 0 0 0 0", "cpu 99 0 100 900 0 0 0 0"),
            ("cpu 100 0 100 800 0 0 0 0", "cpu 100 0 100 800 0 0 0 0"),
            ("cpu 100 0", "cpu 200 0"),
            (
                "cpu 0 0 0 0 0 0 0 0",
                "cpu 18446744073709551615 1 0 0 0 0 0 0",
            ),
        ] {
            assert!(matches!(
                parse_cpu(first, second),
                OperationOutcome::Failed(OperationFailure {
                    kind: OperationFailureKind::InvalidStructuredOutput,
                    ..
                })
            ));
        }
        let guest_columns = parse_cpu(
            "cpu 100 0 100 800 0 0 0 0 9000 8000",
            "cpu 150 0 150 900 0 0 0 0 9900 8800",
        );
        assert!(matches!(
            guest_columns,
            OperationOutcome::Succeeded(OperationSuccess {
                output: ParsedOperationOutput::CpuUsage {
                    basis_points: 5_000,
                    ..
                },
                ..
            })
        ));
    }

    #[test]
    fn timeout_wins_over_an_absent_exit_status() {
        let plan = plan(&RemoteOperation::Network(NetworkOperation::Routes)).unwrap();
        let mut output = raw(None, b"", b"");
        output.timed_out = true;
        assert!(matches!(
            parse_result(&plan, output).unwrap(),
            OperationOutcome::Failed(OperationFailure {
                kind: OperationFailureKind::TimedOut,
                ..
            })
        ));
    }

    #[test]
    fn oversized_capture_is_rejected_even_if_the_transport_forgot_its_limit() {
        let plan = plan(&RemoteOperation::Network(NetworkOperation::Routes)).unwrap();
        let output = raw(Some(0), &vec![b'x'; plan.max_output_bytes + 1], b"");
        assert_eq!(
            parse_result(&plan, output),
            Err(ResultParseError::CapturedOutputExceedsPlan)
        );
    }
}
