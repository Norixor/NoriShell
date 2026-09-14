use std::fmt::Write as _;
use std::time::Duration;

use sha2::{Digest as _, Sha256};

use super::validation::{
    MAX_COMMAND_BYTES, custom_shell, host, http_url, identifier, path, sha256, shell_quote,
};
use super::*;

const MAX_OUTPUT: usize = 64 * 1024;
const MAX_CRONTAB_BYTES: usize = 32 * 1024;

pub fn plan(operation: &RemoteOperation) -> Result<OperationPlan, PlanError> {
    let spec = match operation {
        RemoteOperation::Docker(operation) => docker(operation)?,
        RemoteOperation::Systemd(operation) => systemd(operation)?,
        RemoteOperation::Disk(operation) => disk(operation)?,
        RemoteOperation::Network(operation) => network(operation)?,
        RemoteOperation::Process(operation) => process(operation)?,
        RemoteOperation::Crontab(operation) => crontab(operation)?,
        RemoteOperation::Nginx(operation) => nginx(operation),
        RemoteOperation::Custom(operation) => custom(operation)?,
    };
    finish(spec, review(operation))
}

struct Spec {
    command: String,
    tools: Vec<String>,
    class: OperationClass,
    timeout_seconds: u64,
    max_output_bytes: usize,
    kind: OperationKind,
    recognizes_internal_markers: bool,
    preconditions: Vec<OperationPrecondition>,
    stdin: Option<Vec<u8>>,
    command_is_sensitive: bool,
    stdin_is_sensitive: bool,
}

fn finish(spec: Spec, review: OperationReview) -> Result<OperationPlan, PlanError> {
    let mut command = String::from("LC_ALL=C; export LC_ALL; ");
    for tool in &spec.tools {
        let quoted = shell_quote(tool);
        write!(
            command,
            "command -v {quoted} >/dev/null 2>&1 || {{ printf '%s\\n' 'NORISHELL_OP_ERROR:missing-tool:{tool}' >&2; exit 127; }}; "
        )
        .expect("writing to String cannot fail");
    }
    command.push_str(&spec.command);
    if command.len() > MAX_COMMAND_BYTES {
        return Err(PlanError::CommandTooLarge);
    }
    let approval = if spec.class == OperationClass::Mutation {
        ApprovalRequirement::CoreProtectedPerExecution
    } else {
        ApprovalRequirement::None
    };
    Ok(OperationPlan {
        command,
        stdin: spec.stdin,
        target_os: RemoteOperatingSystem::Linux,
        class: spec.class,
        approval,
        timeout: Duration::from_secs(spec.timeout_seconds),
        max_output_bytes: spec.max_output_bytes,
        kind: spec.kind,
        recognizes_internal_markers: spec.recognizes_internal_markers,
        preconditions: spec.preconditions,
        command_is_sensitive: spec.command_is_sensitive,
        stdin_is_sensitive: spec.stdin_is_sensitive,
        review,
    })
}

fn spec(command: impl Into<String>, tools: &[&str], class: OperationClass) -> Spec {
    Spec {
        command: command.into(),
        tools: tools.iter().map(|tool| (*tool).to_owned()).collect(),
        class,
        timeout_seconds: 15,
        max_output_bytes: MAX_OUTPUT,
        kind: OperationKind::Text,
        recognizes_internal_markers: true,
        preconditions: Vec::new(),
        stdin: None,
        command_is_sensitive: false,
        stdin_is_sensitive: false,
    }
}

fn docker(operation: &DockerOperation) -> Result<Spec, PlanError> {
    let mut result = match operation {
        DockerOperation::List { all } => {
            let all = if *all { " --all" } else { "" };
            spec(
                format!(
                    "docker ps{all} --no-trunc --format '{{{{.ID}}}}\\t{{{{.Names}}}}\\t{{{{.Image}}}}\\t{{{{.Status}}}}\\t{{{{.Ports}}}}'"
                ),
                &["docker"],
                OperationClass::ReadOnly,
            )
        }
        DockerOperation::Stats => spec(
            "docker stats --no-stream --format '{{.ID}}\\t{{.Name}}\\t{{.CPUPerc}}\\t{{.MemUsage}}\\t{{.NetIO}}\\t{{.BlockIO}}'",
            &["docker"],
            OperationClass::ReadOnly,
        ),
        DockerOperation::Logs {
            container_id,
            lines,
            since_seconds,
        } => {
            identifier(container_id, "container_id", 128)?;
            if !(1..=10_000).contains(lines) {
                return Err(PlanError::InvalidParameter { field: "lines" });
            }
            let since =
                since_seconds.map_or_else(String::new, |seconds| format!(" --since {seconds}s"));
            let mut value = spec(
                format!(
                    "docker logs --tail {lines}{since} -- {}",
                    shell_quote(container_id)
                ),
                &["docker"],
                OperationClass::ReadOnly,
            );
            value.timeout_seconds = 30;
            value.max_output_bytes = MAX_OUTPUT;
            value
        }
        DockerOperation::Start { container_id } => {
            identifier(container_id, "container_id", 128)?;
            spec(
                format!("docker start -- {}", shell_quote(container_id)),
                &["docker"],
                OperationClass::Mutation,
            )
        }
        DockerOperation::Stop {
            container_id,
            timeout_seconds,
        } => {
            identifier(container_id, "container_id", 128)?;
            if *timeout_seconds > 300 {
                return Err(PlanError::InvalidParameter {
                    field: "timeout_seconds",
                });
            }
            let mut value = spec(
                format!(
                    "docker stop --time {timeout_seconds} -- {}",
                    shell_quote(container_id)
                ),
                &["docker"],
                OperationClass::Mutation,
            );
            value.timeout_seconds = u64::from(*timeout_seconds) + 15;
            value
        }
        DockerOperation::Restart {
            container_id,
            timeout_seconds,
        } => {
            identifier(container_id, "container_id", 128)?;
            if *timeout_seconds > 300 {
                return Err(PlanError::InvalidParameter {
                    field: "timeout_seconds",
                });
            }
            let mut value = spec(
                format!(
                    "docker restart --time {timeout_seconds} -- {}",
                    shell_quote(container_id)
                ),
                &["docker"],
                OperationClass::Mutation,
            );
            value.timeout_seconds = u64::from(*timeout_seconds) + 30;
            value
        }
    };
    if matches!(operation, DockerOperation::Stats) {
        result.timeout_seconds = 20;
        result.max_output_bytes = MAX_OUTPUT;
    }
    Ok(result)
}

fn systemd(operation: &SystemdOperation) -> Result<Spec, PlanError> {
    let result = match operation {
        SystemdOperation::List => spec(
            "systemctl list-units --type=service --all --no-pager --plain --no-legend",
            &["systemctl"],
            OperationClass::ReadOnly,
        ),
        SystemdOperation::Status { unit } => {
            unit_name(unit)?;
            spec(
                format!(
                    "systemctl status --no-pager --full -- {}; norishell_systemd_status=$?; if [ \"$norishell_systemd_status\" -eq 3 ]; then exit 0; fi; exit \"$norishell_systemd_status\"",
                    shell_quote(unit)
                ),
                &["systemctl"],
                OperationClass::ReadOnly,
            )
        }
        SystemdOperation::Journal { unit, lines } => {
            unit_name(unit)?;
            if !(1..=10_000).contains(lines) {
                return Err(PlanError::InvalidParameter { field: "lines" });
            }
            let mut value = spec(
                format!(
                    "journalctl --no-pager --output=short-iso --lines={lines} --unit={}",
                    shell_quote(unit)
                ),
                &["journalctl"],
                OperationClass::ReadOnly,
            );
            value.timeout_seconds = 30;
            value.max_output_bytes = MAX_OUTPUT;
            value
        }
        SystemdOperation::Start { unit }
        | SystemdOperation::Stop { unit }
        | SystemdOperation::Restart { unit }
        | SystemdOperation::Enable { unit }
        | SystemdOperation::Disable { unit } => {
            unit_name(unit)?;
            let verb = match operation {
                SystemdOperation::Start { .. } => "start",
                SystemdOperation::Stop { .. } => "stop",
                SystemdOperation::Restart { .. } => "restart",
                SystemdOperation::Enable { .. } => "enable",
                SystemdOperation::Disable { .. } => "disable",
                _ => unreachable!(),
            };
            let mut value = spec(
                format!("systemctl {verb} -- {}", shell_quote(unit)),
                &["systemctl"],
                OperationClass::Mutation,
            );
            value.timeout_seconds = 45;
            value
        }
    };
    Ok(result)
}

fn unit_name(unit: &str) -> Result<(), PlanError> {
    identifier(unit, "unit", 256)
}

fn disk(operation: &DiskOperation) -> Result<Spec, PlanError> {
    let mut result = match operation {
        DiskOperation::Filesystems => spec("df -P -B1", &["df"], OperationClass::ReadOnly),
        DiskOperation::Usage {
            path: value,
            max_depth,
        } => {
            path(value)?;
            if *max_depth > 16 {
                return Err(PlanError::InvalidParameter { field: "max_depth" });
            }
            spec(
                format!(
                    "work=$(mktemp -d) || exit 1; raw=\"$work/raw\"; sorted=\"$work/sorted\"; trap 'rm -rf -- \"$work\"' EXIT HUP INT TERM; du -x -B1 --max-depth={max_depth} -- {} >\"$raw\" && sort -nr -- \"$raw\" >\"$sorted\" && head -n 500 -- \"$sorted\"",
                    shell_quote(value)
                ),
                &["du", "mktemp", "rm", "sort", "head"],
                OperationClass::ReadOnly,
            )
        }
        DiskOperation::LargeFiles {
            path: value,
            minimum_bytes,
            limit,
        } => {
            path(value)?;
            if *minimum_bytes == 0 {
                return Err(PlanError::InvalidParameter {
                    field: "minimum_bytes",
                });
            }
            if !(1..=1000).contains(limit) {
                return Err(PlanError::InvalidParameter { field: "limit" });
            }
            let mut value = spec(
                format!(
                    "work=$(mktemp -d) || exit 1; raw=\"$work/raw\"; sorted=\"$work/sorted\"; trap 'rm -rf -- \"$work\"' EXIT HUP INT TERM; find -- {} -xdev -type f -size +{}c -printf '%s\\t%p\\n' >\"$raw\" && sort -nr -- \"$raw\" >\"$sorted\" && head -n {} -- \"$sorted\"",
                    shell_quote(value),
                    minimum_bytes.saturating_sub(1),
                    limit
                ),
                &["mktemp", "rm", "find", "sort", "head"],
                OperationClass::ReadOnly,
            );
            value.timeout_seconds = 60;
            value.max_output_bytes = MAX_OUTPUT;
            value
        }
    };
    if matches!(operation, DiskOperation::Usage { .. }) {
        result.timeout_seconds = 45;
        result.max_output_bytes = MAX_OUTPUT;
    }
    Ok(result)
}

fn network(operation: &NetworkOperation) -> Result<Spec, PlanError> {
    let result = match operation {
        NetworkOperation::Dns { host: value } => {
            host(value, "host")?;
            spec(
                format!("getent ahosts -- {}", shell_quote(value)),
                &["getent"],
                OperationClass::ReadOnly,
            )
        }
        NetworkOperation::Tcp {
            host: value,
            port,
            timeout_seconds,
        } => {
            host(value, "host")?;
            port_and_timeout(*port, *timeout_seconds)?;
            spec(
                format!(
                    "timeout {} nc -z -v -w {} -- {} {}",
                    timeout_seconds,
                    timeout_seconds,
                    shell_quote(value),
                    port
                ),
                &["timeout", "nc"],
                OperationClass::ReadOnly,
            )
        }
        NetworkOperation::Http {
            url,
            timeout_seconds,
        } => {
            http_url(url)?;
            bounded_timeout(*timeout_seconds)?;
            let mut value = spec(
                format!(
                    "curl --disable --silent --show-error --include --max-time {timeout_seconds} --max-filesize {} --write-out {} --url {}",
                    MAX_OUTPUT,
                    shell_quote(
                        "\nNORISHELL_HTTP_METRICS status=%{http_code} remote_ip=%{remote_ip} dns=%{time_namelookup} connect=%{time_connect} tls=%{time_appconnect} first_byte=%{time_starttransfer} total=%{time_total}\n"
                    ),
                    shell_quote(url)
                ),
                &["curl"],
                OperationClass::ReadOnly,
            );
            value.timeout_seconds = u64::from(*timeout_seconds) + 5;
            value.max_output_bytes = MAX_OUTPUT;
            value.command_is_sensitive = url.contains('?') || url.contains('#');
            value
        }
        NetworkOperation::Routes => {
            spec("ip -details route show", &["ip"], OperationClass::ReadOnly)
        }
        NetworkOperation::RouteTo { destination } => {
            host(destination, "destination")?;
            spec(
                format!("ip route get {}", shell_quote(destination)),
                &["ip"],
                OperationClass::ReadOnly,
            )
        }
        NetworkOperation::Tls {
            host: value,
            port,
            server_name,
            timeout_seconds,
        } => {
            host(value, "host")?;
            port_and_timeout(*port, *timeout_seconds)?;
            if let Some(server_name) = server_name {
                host(server_name, "server_name")?;
            }
            let connect = if value.contains(':') {
                format!("[{value}]:{port}")
            } else {
                format!("{value}:{port}")
            };
            let server_name = server_name.as_deref().unwrap_or(value);
            let mut plan = spec(
                format!(
                    "timeout {timeout_seconds} openssl s_client -brief -connect {} -servername {} </dev/null",
                    shell_quote(&connect),
                    shell_quote(server_name)
                ),
                &["timeout", "openssl"],
                OperationClass::ReadOnly,
            );
            plan.timeout_seconds = u64::from(*timeout_seconds) + 5;
            plan
        }
    };
    Ok(result)
}

fn port_and_timeout(port: u16, timeout_seconds: u8) -> Result<(), PlanError> {
    if port == 0 {
        return Err(PlanError::InvalidParameter { field: "port" });
    }
    bounded_timeout(timeout_seconds)
}

fn bounded_timeout(timeout_seconds: u8) -> Result<(), PlanError> {
    if !(1..=60).contains(&timeout_seconds) {
        return Err(PlanError::InvalidParameter {
            field: "timeout_seconds",
        });
    }
    Ok(())
}

fn process(operation: &ProcessOperation) -> Result<Spec, PlanError> {
    let result = match operation {
        ProcessOperation::List { sort } => {
            let sort = match sort {
                ProcessSort::Cpu => "-pcpu",
                ProcessSort::Memory => "-rss",
            };
            spec(
                format!(
                    "ps -eo pid=,ppid=,user=,stat=,pcpu=,pmem=,rss=,etimes=,comm= --sort={sort}"
                ),
                &["ps"],
                OperationClass::ReadOnly,
            )
        }
        ProcessOperation::Sockets => {
            let mut value = spec("ss -H -n -l -t -u -p", &["ss"], OperationClass::ReadOnly);
            value.max_output_bytes = MAX_OUTPUT;
            value
        }
        ProcessOperation::Detail { pid } => {
            valid_pid(*pid)?;
            let command = process_identity_prefix(*pid, None);
            let mut value = spec(
                format!(
                    "{command}printf 'NORISHELL_PROCESS_IDENTITY:%s:%s\\n' \"$pid\" \"$actual_start\"; ps -p \"$pid\" -o pid=,ppid=,uid=,user=,stat=,pcpu=,pmem=,rss=,lstart=,etimes=,comm= || exit $?; if command -v systemctl >/dev/null 2>&1; then unit=$(awk -F/ '{{ for (i = NF; i >= 1; i--) if ($i ~ /[.]service$/) {{ print $i; exit }} }}' \"/proc/$pid/cgroup\"); if [ -n \"$unit\" ]; then printf 'NORISHELL_PROCESS_SYSTEMD:available:%s\\n' \"$unit\"; else printf '%s\\n' 'NORISHELL_PROCESS_SYSTEMD:not-found'; fi; else printf '%s\\n' 'NORISHELL_PROCESS_SYSTEMD:unavailable'; fi"
                ),
                &["cat", "ps", "awk"],
                OperationClass::ReadOnly,
            );
            value.kind = OperationKind::ProcessSnapshot;
            value
        }
        ProcessOperation::CpuUsage {} => {
            let mut value = spec(
                "IFS= read -r first < /proc/stat || exit 1; printf 'NORISHELL_CPU_STAT:first:%s\\n' \"$first\"; sleep 0.2; IFS= read -r second < /proc/stat || exit 1; printf 'NORISHELL_CPU_STAT:second:%s\\n' \"$second\"",
                &["sleep"],
                OperationClass::ReadOnly,
            );
            value.timeout_seconds = 2;
            value.max_output_bytes = CPU_USAGE_MAX_OUTPUT_BYTES;
            value.kind = OperationKind::CpuUsage;
            value
        }
        ProcessOperation::Signal { identity, signal } => {
            valid_pid(identity.pid)?;
            if identity.start_time_ticks == 0 {
                return Err(PlanError::InvalidParameter {
                    field: "start_time_ticks",
                });
            }
            let prefix = process_identity_prefix(identity.pid, Some(identity.start_time_ticks));
            let signal = match signal {
                ProcessSignal::Term => "TERM",
                ProcessSignal::Kill => "KILL",
            };
            let mut value = spec(
                format!("{prefix}kill -{signal} \"$pid\""),
                &["cat", "kill"],
                OperationClass::Mutation,
            );
            value
                .preconditions
                .push(OperationPrecondition::ProcessIdentity(identity.clone()));
            value
        }
    };
    Ok(result)
}

fn valid_pid(pid: u32) -> Result<(), PlanError> {
    if pid < 2 || pid > i32::MAX as u32 {
        return Err(PlanError::InvalidParameter { field: "pid" });
    }
    Ok(())
}

fn process_identity_prefix(pid: u32, expected: Option<u64>) -> String {
    let mut command = format!(
        "pid={pid}; stat=$(cat -- \"/proc/$pid/stat\") || {{ printf '%s\\n' 'NORISHELL_OP_ERROR:process-identity-mismatch' >&2; exit 73; }}; set -- ${{stat##*) }}; actual_start=${{20-}}; [ -n \"$actual_start\" ] || {{ printf '%s\\n' 'NORISHELL_OP_ERROR:process-identity-mismatch' >&2; exit 73; }}; "
    );
    if let Some(expected) = expected {
        write!(command, "[ \"$actual_start\" = {expected} ] || {{ printf '%s\\n' 'NORISHELL_OP_ERROR:process-identity-mismatch' >&2; exit 73; }}; ").expect("writing to String cannot fail");
    }
    command
}

fn crontab(operation: &CrontabOperation) -> Result<Spec, PlanError> {
    match operation {
        CrontabOperation::Read => {
            let mut value = spec(
                format!(
                    "{}{}hash=$(sha256sum \"$current\"); hash=${{hash%% *}}; printf 'NORISHELL_CRONTAB_SHA256:%s\\n' \"$hash\"; cat -- \"$current\"",
                    crontab_work_prefix(),
                    crontab_read_current()
                ),
                &["crontab", "mktemp", "rm", "grep", "sha256sum", "cat"],
                OperationClass::ReadOnly,
            );
            value.kind = OperationKind::CrontabSnapshot;
            value.max_output_bytes = MAX_OUTPUT;
            Ok(value)
        }
        CrontabOperation::Replace {
            expected_sha256,
            content,
        } => {
            let expected_sha256 = sha256(expected_sha256, "expected_sha256")?;
            if content.len() > MAX_CRONTAB_BYTES {
                return Err(PlanError::ParameterTooLarge {
                    field: "content",
                    maximum: MAX_CRONTAB_BYTES,
                });
            }
            if content.bytes().any(|byte| byte == 0) {
                return Err(PlanError::InvalidParameter { field: "content" });
            }
            let replacement_sha256 = format!("{:x}", Sha256::digest(content.as_bytes()));
            let mut value = spec(
                format!(
                    "{}replacement=\"$work/replacement\"; cat >\"$replacement\" || exit 1; replacement_hash=$(sha256sum \"$replacement\"); replacement_hash=${{replacement_hash%% *}}; [ \"$replacement_hash\" = {} ] || exit 74; {}current_hash=$(sha256sum \"$current\"); current_hash=${{current_hash%% *}}; [ \"$current_hash\" = {} ] || {{ printf '%s\\n' 'NORISHELL_OP_ERROR:crontab-cas-mismatch' >&2; exit 73; }}; crontab \"$replacement\"",
                    crontab_work_prefix(),
                    shell_quote(&replacement_sha256),
                    crontab_read_current(),
                    shell_quote(&expected_sha256),
                ),
                &["crontab", "mktemp", "rm", "grep", "sha256sum", "cat"],
                OperationClass::Mutation,
            );
            value.timeout_seconds = 20;
            value.max_output_bytes = MAX_OUTPUT;
            value
                .preconditions
                .push(OperationPrecondition::CrontabSha256(expected_sha256));
            value.stdin = Some(content.as_bytes().to_vec());
            value.stdin_is_sensitive = true;
            Ok(value)
        }
    }
}

fn crontab_work_prefix() -> &'static str {
    "umask 077; work=$(mktemp -d) || exit 1; current=\"$work/current\"; error=\"$work/error\"; trap 'rm -rf -- \"$work\"' EXIT HUP INT TERM; "
}

fn crontab_read_current() -> &'static str {
    "if crontab -l >\"$current\" 2>\"$error\"; then :; elif grep -q '^no crontab for ' \"$error\"; then : >\"$current\"; else cat -- \"$error\" >&2; exit 1; fi; "
}

fn nginx(operation: &NginxOperation) -> Spec {
    match operation {
        NginxOperation::Sites => spec(
            "find -- /etc/nginx/sites-enabled -mindepth 1 -maxdepth 1 -printf '%f\\t%y\\t%l\\n'",
            &["find"],
            OperationClass::ReadOnly,
        ),
        NginxOperation::ConfigTest => {
            let mut value = spec("nginx -t", &["nginx"], OperationClass::ReadOnly);
            value.timeout_seconds = 20;
            value
        }
        NginxOperation::Reload => {
            let mut value = spec(
                "nginx -t; result=$?; [ \"$result\" -eq 0 ] || { printf '%s\\n' 'NORISHELL_OP_ERROR:nginx-config-test-failed' >&2; exit \"$result\"; }; nginx -s reload",
                &["nginx"],
                OperationClass::Mutation,
            );
            value.timeout_seconds = 30;
            value
                .preconditions
                .push(OperationPrecondition::NginxConfigTest);
            value
        }
    }
}

fn custom(operation: &CustomOperation) -> Result<Spec, PlanError> {
    match operation {
        CustomOperation::Shell {
            command,
            timeout_seconds,
            max_output_bytes,
        } => {
            custom_shell(command)?;
            if !(1..=120).contains(timeout_seconds) {
                return Err(PlanError::InvalidParameter {
                    field: "timeout_seconds",
                });
            }
            let max_output_bytes =
                usize::try_from(*max_output_bytes).map_err(|_| PlanError::InvalidParameter {
                    field: "max_output_bytes",
                })?;
            if !(1024..=MAX_OUTPUT).contains(&max_output_bytes) {
                return Err(PlanError::InvalidParameter {
                    field: "max_output_bytes",
                });
            }
            let mut value = spec(command.clone(), &[], OperationClass::Mutation);
            value.recognizes_internal_markers = false;
            value.command_is_sensitive = true;
            value.timeout_seconds = u64::from(*timeout_seconds);
            value.max_output_bytes = max_output_bytes;
            Ok(value)
        }
    }
}

fn review(operation: &RemoteOperation) -> OperationReview {
    let (action, target, detail) = match operation {
        RemoteOperation::Docker(operation) => match operation {
            DockerOperation::List { all } => ("docker.list", None, Some(format!("all={all}"))),
            DockerOperation::Stats => ("docker.stats", None, None),
            DockerOperation::Logs {
                container_id,
                lines,
                since_seconds,
            } => (
                "docker.logs",
                Some(container_id.clone()),
                Some(format!("lines={lines},sinceSeconds={since_seconds:?}")),
            ),
            DockerOperation::Start { container_id } => {
                ("docker.start", Some(container_id.clone()), None)
            }
            DockerOperation::Stop {
                container_id,
                timeout_seconds,
            } => (
                "docker.stop",
                Some(container_id.clone()),
                Some(format!("timeoutSeconds={timeout_seconds}")),
            ),
            DockerOperation::Restart {
                container_id,
                timeout_seconds,
            } => (
                "docker.restart",
                Some(container_id.clone()),
                Some(format!("timeoutSeconds={timeout_seconds}")),
            ),
        },
        RemoteOperation::Systemd(operation) => match operation {
            SystemdOperation::List => ("systemd.list", None, None),
            SystemdOperation::Status { unit } => ("systemd.status", Some(unit.clone()), None),
            SystemdOperation::Journal { unit, lines } => (
                "systemd.journal",
                Some(unit.clone()),
                Some(format!("lines={lines}")),
            ),
            SystemdOperation::Start { unit } => ("systemd.start", Some(unit.clone()), None),
            SystemdOperation::Stop { unit } => ("systemd.stop", Some(unit.clone()), None),
            SystemdOperation::Restart { unit } => ("systemd.restart", Some(unit.clone()), None),
            SystemdOperation::Enable { unit } => ("systemd.enable", Some(unit.clone()), None),
            SystemdOperation::Disable { unit } => ("systemd.disable", Some(unit.clone()), None),
        },
        RemoteOperation::Disk(operation) => match operation {
            DiskOperation::Filesystems => ("disk.filesystems", None, None),
            DiskOperation::Usage { path, max_depth } => (
                "disk.usage",
                Some(path.clone()),
                Some(format!("maxDepth={max_depth}")),
            ),
            DiskOperation::LargeFiles {
                path,
                minimum_bytes,
                limit,
            } => (
                "disk.largeFiles",
                Some(path.clone()),
                Some(format!("minimumBytes={minimum_bytes},limit={limit}")),
            ),
        },
        RemoteOperation::Network(operation) => match operation {
            NetworkOperation::Dns { host } => ("network.dns", Some(host.clone()), None),
            NetworkOperation::Tcp {
                host,
                port,
                timeout_seconds,
            } => (
                "network.tcp",
                Some(format!("{host}:{port}")),
                Some(format!("timeoutSeconds={timeout_seconds}")),
            ),
            NetworkOperation::Http {
                url,
                timeout_seconds,
            } => (
                "network.http",
                Some(url.split(['?', '#']).next().unwrap_or_default().to_owned()),
                Some(format!("timeoutSeconds={timeout_seconds}")),
            ),
            NetworkOperation::Routes => ("network.routes", None, None),
            NetworkOperation::RouteTo { destination } => {
                ("network.routeTo", Some(destination.clone()), None)
            }
            NetworkOperation::Tls {
                host,
                port,
                server_name,
                timeout_seconds,
            } => (
                "network.tls",
                Some(format!("{host}:{port}")),
                Some(format!(
                    "serverName={server_name:?},timeoutSeconds={timeout_seconds}"
                )),
            ),
        },
        RemoteOperation::Process(operation) => match operation {
            ProcessOperation::List { sort } => {
                ("process.list", None, Some(format!("sort={sort:?}")))
            }
            ProcessOperation::Sockets => ("process.sockets", None, None),
            ProcessOperation::Detail { pid } => ("process.detail", Some(pid.to_string()), None),
            ProcessOperation::CpuUsage {} => ("process.cpuUsage", None, None),
            ProcessOperation::Signal { identity, signal } => (
                "process.signal",
                Some(identity.pid.to_string()),
                Some(format!(
                    "signal={signal:?},startTimeTicks={}",
                    identity.start_time_ticks
                )),
            ),
        },
        RemoteOperation::Crontab(operation) => match operation {
            CrontabOperation::Read => ("crontab.read", None, None),
            CrontabOperation::Replace { content, .. } => (
                "crontab.replace",
                None,
                Some(format!(
                    "replacementSha256={:x}",
                    Sha256::digest(content.as_bytes())
                )),
            ),
        },
        RemoteOperation::Nginx(operation) => match operation {
            NginxOperation::Sites => ("nginx.sites", None, None),
            NginxOperation::ConfigTest => ("nginx.configTest", None, None),
            NginxOperation::Reload => (
                "nginx.reload",
                None,
                Some("requiresConfigTest=true".to_owned()),
            ),
        },
        RemoteOperation::Custom(CustomOperation::Shell { command, .. }) => {
            ("custom.shell", None, Some(command.clone()))
        }
    };
    OperationReview {
        action,
        target,
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_wire_is_strict_and_camel_case() {
        let operation = RemoteOperation::Docker(DockerOperation::Logs {
            container_id: "web-1".to_owned(),
            lines: 200,
            since_seconds: None,
        });
        assert_eq!(
            serde_json::to_value(&operation).unwrap(),
            serde_json::json!({
                "kind": "docker",
                "operation": {"kind": "logs", "containerId": "web-1", "lines": 200, "sinceSeconds": null}
            })
        );
        assert!(
            serde_json::from_value::<RemoteOperation>(serde_json::json!({
                "kind": "docker",
                "operation": {"kind": "list", "all": true, "unexpected": true}
            }))
            .is_err()
        );
    }

    #[test]
    fn typed_identifier_rejects_shell_and_option_injection() {
        assert!(matches!(
            plan(&RemoteOperation::Docker(DockerOperation::Start {
                container_id: "web; id".to_owned(),
            })),
            Err(PlanError::InvalidParameter {
                field: "container_id"
            })
        ));
        assert!(matches!(
            plan(&RemoteOperation::Network(NetworkOperation::Dns {
                host: "--help".to_owned(),
            })),
            Err(PlanError::InvalidParameter { field: "host" })
        ));
    }

    #[test]
    fn path_is_shell_quoted_and_never_becomes_an_option() {
        let planned = plan(&RemoteOperation::Disk(DiskOperation::Usage {
            path: "/srv/a'b; printf injected".to_owned(),
            max_depth: 2,
        }))
        .unwrap();
        assert!(
            planned
                .command
                .contains("-- '/srv/a'\"'\"'b; printf injected'")
        );
        assert_eq!(planned.class, OperationClass::ReadOnly);
        assert_eq!(planned.max_output_bytes, MAX_OUTPUT);
    }

    #[cfg(unix)]
    #[test]
    fn systemd_status_accepts_inactive_but_preserves_missing_and_other_failures() {
        use std::{fs, os::unix::fs::PermissionsExt as _, process::Command};

        let directory = tempfile::tempdir().unwrap();
        let systemctl = directory.path().join("systemctl");
        fs::write(
            &systemctl,
            "#!/bin/sh\nprintf 'fixture status output\\n'\nexit \"$FIXTURE_EXIT\"\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&systemctl).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&systemctl, permissions).unwrap();
        let planned = plan(&RemoteOperation::Systemd(SystemdOperation::Status {
            unit: "dpkg-db-backup.service".to_owned(),
        }))
        .unwrap();

        #[cfg(target_os = "macos")]
        let shells = ["/bin/sh", "/bin/zsh"].as_slice();
        #[cfg(not(target_os = "macos"))]
        let shells = ["/bin/sh"].as_slice();
        for shell in shells {
            for (systemctl_exit, expected_exit) in [(0, 0), (3, 0), (4, 4), (1, 1)] {
                let output = Command::new(shell)
                    .args(["-c", &planned.command])
                    .env("PATH", directory.path())
                    .env("FIXTURE_EXIT", systemctl_exit.to_string())
                    .output()
                    .unwrap();
                assert_eq!(
                    output.status.code(),
                    Some(expected_exit),
                    "unexpected classification for systemctl exit {systemctl_exit}"
                );
                assert_eq!(output.stdout, b"fixture status output\n");
            }
        }
    }

    #[test]
    fn every_fixed_mutation_requires_fresh_core_approval() {
        let mutations = [
            RemoteOperation::Docker(DockerOperation::Restart {
                container_id: "api".to_owned(),
                timeout_seconds: 10,
            }),
            RemoteOperation::Systemd(SystemdOperation::Disable {
                unit: "demo.service".to_owned(),
            }),
            RemoteOperation::Process(ProcessOperation::Signal {
                identity: ProcessIdentity {
                    pid: 42,
                    start_time_ticks: 1234,
                },
                signal: ProcessSignal::Term,
            }),
            RemoteOperation::Nginx(NginxOperation::Reload),
        ];
        for operation in mutations {
            let planned = plan(&operation).unwrap();
            assert_eq!(planned.class, OperationClass::Mutation);
            assert_eq!(
                planned.approval,
                ApprovalRequirement::CoreProtectedPerExecution
            );
        }
    }

    #[test]
    fn custom_shell_supports_pipelines_but_is_always_reviewed() {
        let command = "printf '%s\\n' alpha | sed 's/a/A/g'";
        let planned = plan(&RemoteOperation::Custom(CustomOperation::Shell {
            command: command.to_owned(),
            timeout_seconds: 10,
            max_output_bytes: 8192,
        }))
        .unwrap();
        assert!(planned.command.ends_with(command));
        assert_eq!(
            planned.approval,
            ApprovalRequirement::CoreProtectedPerExecution
        );
        assert_eq!(planned.review.detail.as_deref(), Some(command));
    }

    #[test]
    fn custom_shell_rejects_known_secret_material() {
        assert!(matches!(
            plan(&RemoteOperation::Custom(CustomOperation::Shell {
                command: "printf '%s' '-----BEGIN OPENSSH PRIVATE KEY-----'".to_owned(),
                timeout_seconds: 10,
                max_output_bytes: 4096,
            })),
            Err(PlanError::KnownSecretMaterial)
        ));
    }

    #[test]
    fn crontab_replacement_uses_stdin_and_full_content_hashes() {
        let content = "MAILTO=ops@example.com\n*/5 * * * * /usr/bin/true\n";
        let planned = plan(&RemoteOperation::Crontab(CrontabOperation::Replace {
            expected_sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                .to_owned(),
            content: content.to_owned(),
        }))
        .unwrap();
        assert_eq!(planned.stdin.as_deref(), Some(content.as_bytes()));
        assert!(planned.stdin_is_sensitive);
        assert!(!planned.command.contains(content));
        assert!(planned.command.contains("crontab-cas-mismatch"));
        assert!(planned.command.contains("replacement_hash"));
        assert!(planned.command.contains("mktemp -d"));
        assert_eq!(planned.preconditions.len(), 1);
    }

    #[test]
    fn process_signal_rechecks_linux_start_time_before_kill() {
        let planned = plan(&RemoteOperation::Process(ProcessOperation::Signal {
            identity: ProcessIdentity {
                pid: 77,
                start_time_ticks: 9988,
            },
            signal: ProcessSignal::Kill,
        }))
        .unwrap();
        let check = planned.command.find("actual_start").unwrap();
        let kill = planned.command.rfind("kill -KILL").unwrap();
        assert!(check < kill);
        assert!(planned.command.contains("= 9988"));
    }

    #[test]
    fn nginx_reload_tests_configuration_first() {
        let planned = plan(&RemoteOperation::Nginx(NginxOperation::Reload)).unwrap();
        assert!(
            planned.command.find("nginx -t").unwrap()
                < planned.command.find("nginx -s reload").unwrap()
        );
        assert_eq!(
            planned.preconditions,
            vec![OperationPrecondition::NginxConfigTest]
        );
    }

    #[test]
    fn process_and_disk_rankings_are_explicit_and_bounded() {
        let cpu = plan(&RemoteOperation::Process(ProcessOperation::List {
            sort: ProcessSort::Cpu,
        }))
        .unwrap();
        let memory = plan(&RemoteOperation::Process(ProcessOperation::List {
            sort: ProcessSort::Memory,
        }))
        .unwrap();
        let disk = plan(&RemoteOperation::Disk(DiskOperation::Usage {
            path: "/".to_owned(),
            max_depth: 2,
        }))
        .unwrap();
        assert!(cpu.command.contains("pcpu=") && cpu.command.contains("--sort=-pcpu"));
        assert!(memory.command.contains("rss=") && memory.command.contains("--sort=-rss"));
        assert!(disk.command.contains("sort -nr") && disk.command.contains("head -n 500"));
        assert!(cpu.max_output_bytes <= 64 * 1024 && disk.max_output_bytes <= 64 * 1024);
    }

    #[test]
    fn cpu_usage_is_a_fixed_short_read_only_procfs_sample() {
        assert_eq!(
            serde_json::to_value(RemoteOperation::Process(ProcessOperation::CpuUsage {})).unwrap(),
            serde_json::json!({"kind": "process", "operation": {"kind": "cpuUsage"}})
        );
        assert!(
            serde_json::from_value::<RemoteOperation>(serde_json::json!({
                "kind": "process",
                "operation": {"kind": "cpuUsage", "intervalSeconds": 1},
            }))
            .is_err()
        );
        let planned = plan(&RemoteOperation::Process(ProcessOperation::CpuUsage {})).unwrap();
        assert_eq!(planned.class, OperationClass::ReadOnly);
        assert_eq!(planned.approval, ApprovalRequirement::None);
        assert_eq!(planned.kind, OperationKind::CpuUsage);
        assert_eq!(planned.timeout, Duration::from_secs(2));
        assert_eq!(planned.max_output_bytes, CPU_USAGE_MAX_OUTPUT_BYTES);
        assert!(planned.stdin.is_none());
        assert_eq!(planned.command.matches("/proc/stat").count(), 2);
        assert!(planned.command.contains("sleep 0.2"));
        assert!(planned.command.contains("NORISHELL_CPU_STAT:first:"));
        assert!(planned.command.contains("NORISHELL_CPU_STAT:second:"));
        assert!(!planned.command.contains("ps "));
        assert!(!planned.command.contains("sudo"));
    }
}
