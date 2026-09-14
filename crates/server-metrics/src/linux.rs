use std::{collections::BTreeMap, str, time::Duration};

use thiserror::Error;

pub const LINUX_PROVIDER_ID: &str = "linux-procfs";
pub const LINUX_PROVIDER_VERSION: u16 = 1;

pub const MAX_PROC_STAT_BYTES: usize = 4 * 1024;
pub const MAX_MEMINFO_BYTES: usize = 64 * 1024;
pub const MAX_NET_DEV_BYTES: usize = 64 * 1024;
pub const MAX_DF_BYTES: usize = 16 * 1024;

const MAX_PROC_STAT_LINES: usize = 2;
const MAX_MEMINFO_LINES: usize = 256;
const MAX_NET_DEV_LINES: usize = 128;
const MAX_DF_LINES: usize = 8;
const MAX_CPU_FIELDS: usize = 16;
const NETWORK_FIELD_COUNT: usize = 16;
const MAX_DF_FIELDS: usize = 32;
const MAX_INTERFACE_NAME_BYTES: usize = 64;
const MAX_FILESYSTEM_ID_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxMetricSource {
    ProcStat,
    ProcMeminfo,
    ProcNetDev,
    RootDisk,
}

pub const MAX_PLATFORM_BYTES: usize = 128;
pub const MAX_LINUX_PROBE_BYTES: usize = MAX_PLATFORM_BYTES
    + MAX_PROC_STAT_BYTES
    + MAX_MEMINFO_BYTES
    + MAX_NET_DEV_BYTES
    + MAX_DF_BYTES
    + 4;

/// One fixed, NUL-framed probe replaces five sequential SSH exec channels. The first section is
/// the platform name. Linux adds aggregate CPU, memory, network, and root-disk sections. The
/// command is compiled into the provider, contains no user input, requests no PTY, and uses no
/// sudo. A non-Linux host exits successfully after the platform section.
pub const LINUX_PROBE_COMMAND: &[u8] = b"export LC_ALL=C; uname -s || exit 10; printf '\\000' || exit 11; uname -s | grep -qx Linux || exit 0; head -n 1 /proc/stat || exit 20; printf '\\000' || exit 21; cat /proc/meminfo || exit 30; printf '\\000' || exit 31; cat /proc/net/dev || exit 40; printf '\\000' || exit 41; df -Pk -- / || exit 50";

/// A first successful provider sample has no prior CPU counters. Reusing the same authenticated
/// Metrics transport for one short follow-up exec makes CPU available in the first Probe without
/// retaining a socket between scheduled samples.
pub const LINUX_CPU_FOLLOW_UP_COMMAND: &[u8] = b"LC_ALL=C head -n 1 /proc/stat";

#[derive(Debug, Clone, Copy)]
pub struct LinuxRawSample<'a> {
    pub proc_stat: &'a [u8],
    pub proc_meminfo: &'a [u8],
    pub proc_net_dev: &'a [u8],
    pub root_df: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricUnavailableReason {
    InitialBaseline,
    CounterReset,
    CounterSetChanged,
    NoCounterProgress,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetricValue<T> {
    Available(T),
    Unavailable(MetricUnavailableReason),
}

impl<T> MetricValue<T> {
    #[must_use]
    pub const fn is_available(&self) -> bool {
        matches!(self, Self::Available(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuUsage {
    /// CPU use in hundredths of a percent, from 0 through 10,000.
    pub basis_points: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryUsage {
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkRate {
    pub receive_bytes_per_second: u64,
    pub transmit_bytes_per_second: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskUsage {
    pub filesystem_id: String,
    pub mount: String,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxMetricSample {
    pub provider_id: &'static str,
    pub provider_version: u16,
    pub sampled_at: Duration,
    pub cpu: MetricValue<CpuUsage>,
    pub memory: MemoryUsage,
    pub network: MetricValue<NetworkRate>,
    pub root_disk: DiskUsage,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ParseError {
    #[error("{metric_source:?} output exceeds the {max_bytes}-byte limit")]
    InputTooLarge {
        metric_source: LinuxMetricSource,
        max_bytes: usize,
    },
    #[error("{metric_source:?} output exceeds the {max_lines}-line limit")]
    TooManyLines {
        metric_source: LinuxMetricSource,
        max_lines: usize,
    },
    #[error("{0:?} output is not valid UTF-8")]
    InvalidUtf8(LinuxMetricSource),
    #[error("{metric_source:?} output is malformed: {detail}")]
    Malformed {
        metric_source: LinuxMetricSource,
        detail: &'static str,
    },
    #[error("{metric_source:?} output contains a duplicate {field} field")]
    DuplicateField {
        metric_source: LinuxMetricSource,
        field: &'static str,
    },
    #[error("{metric_source:?} output is missing the {field} field")]
    MissingField {
        metric_source: LinuxMetricSource,
        field: &'static str,
    },
    #[error("{metric_source:?} output contains an invalid or overflowing number")]
    InvalidNumber { metric_source: LinuxMetricSource },
    #[error("metric arithmetic overflowed")]
    ArithmeticOverflow,
    #[error("sample time must increase monotonically")]
    NonMonotonicSampleTime,
}

#[derive(Debug, Clone, Default)]
pub struct LinuxMetricsProvider {
    baseline: Option<Baseline>,
}

impl LinuxMetricsProvider {
    #[must_use]
    pub const fn new() -> Self {
        Self { baseline: None }
    }

    /// Parses one complete provider sample and updates CPU/network baselines atomically.
    ///
    /// A malformed source leaves the prior baseline unchanged. The first valid sample only creates
    /// CPU/network baselines. Counter resets and interface-set changes rebuild the relevant
    /// baseline and return an explicit unavailable value for that interval.
    pub fn sample(
        &mut self,
        sampled_at: Duration,
        raw: LinuxRawSample<'_>,
    ) -> Result<LinuxMetricSample, ParseError> {
        let cpu_counters = parse_proc_stat(raw.proc_stat)?;
        let memory = parse_meminfo(raw.proc_meminfo)?;
        let network_counters = parse_net_dev(raw.proc_net_dev)?;
        let root_disk = parse_root_df(raw.root_df)?;

        let (cpu, network) = match &self.baseline {
            Some(previous) => {
                let elapsed = sampled_at
                    .checked_sub(previous.sampled_at)
                    .filter(|elapsed| !elapsed.is_zero())
                    .ok_or(ParseError::NonMonotonicSampleTime)?;
                (
                    calculate_cpu(&previous.cpu, &cpu_counters)?,
                    calculate_network(&previous.network, &network_counters, elapsed)?,
                )
            }
            None => (
                MetricValue::Unavailable(MetricUnavailableReason::InitialBaseline),
                MetricValue::Unavailable(MetricUnavailableReason::InitialBaseline),
            ),
        };

        self.baseline = Some(Baseline {
            sampled_at,
            cpu: cpu_counters,
            network: network_counters,
        });

        Ok(LinuxMetricSample {
            provider_id: LINUX_PROVIDER_ID,
            provider_version: LINUX_PROVIDER_VERSION,
            sampled_at,
            cpu,
            memory,
            network,
            root_disk,
        })
    }

    /// Completes an initial CPU window without changing the network sample time or counters.
    /// Invalid output leaves the existing baseline untouched; resets rebuild only the CPU side.
    pub fn sample_cpu_follow_up(
        &mut self,
        proc_stat: &[u8],
    ) -> Result<MetricValue<CpuUsage>, ParseError> {
        let current = parse_proc_stat(proc_stat)?;
        let Some(baseline) = self.baseline.as_mut() else {
            return Ok(MetricValue::Unavailable(
                MetricUnavailableReason::InitialBaseline,
            ));
        };
        let cpu = calculate_cpu(&baseline.cpu, &current)?;
        baseline.cpu = current;
        Ok(cpu)
    }

    pub fn reset_baseline(&mut self) {
        self.baseline = None;
    }
}

#[derive(Debug, Clone)]
struct Baseline {
    sampled_at: Duration,
    cpu: CpuCounters,
    network: BTreeMap<String, InterfaceCounters>,
}

#[derive(Debug, Clone)]
struct CpuCounters {
    fields: [u64; 8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InterfaceCounters {
    receive_bytes: u64,
    transmit_bytes: u64,
}

fn parse_proc_stat(input: &[u8]) -> Result<CpuCounters, ParseError> {
    let text = bounded_text(
        LinuxMetricSource::ProcStat,
        input,
        MAX_PROC_STAT_BYTES,
        MAX_PROC_STAT_LINES,
    )?;
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let line = lines.next().ok_or(ParseError::MissingField {
        metric_source: LinuxMetricSource::ProcStat,
        field: "cpu",
    })?;
    if lines.next().is_some() {
        return Err(malformed(
            LinuxMetricSource::ProcStat,
            "expected exactly one aggregate cpu line",
        ));
    }

    let mut tokens = line.split_ascii_whitespace();
    if tokens.next() != Some("cpu") {
        return Err(malformed(
            LinuxMetricSource::ProcStat,
            "aggregate cpu line is missing",
        ));
    }
    let values = tokens
        .map(|token| parse_u64(LinuxMetricSource::ProcStat, token))
        .collect::<Result<Vec<_>, _>>()?;
    if values.len() < 8 || values.len() > MAX_CPU_FIELDS {
        return Err(malformed(
            LinuxMetricSource::ProcStat,
            "aggregate cpu field count is outside the provider contract",
        ));
    }
    let fields: [u64; 8] = values[..8].try_into().map_err(|_| {
        malformed(
            LinuxMetricSource::ProcStat,
            "aggregate cpu fields are invalid",
        )
    })?;
    checked_sum(LinuxMetricSource::ProcStat, &fields)?;
    Ok(CpuCounters { fields })
}

fn parse_meminfo(input: &[u8]) -> Result<MemoryUsage, ParseError> {
    let text = bounded_text(
        LinuxMetricSource::ProcMeminfo,
        input,
        MAX_MEMINFO_BYTES,
        MAX_MEMINFO_LINES,
    )?;
    let mut total_kib = None;
    let mut available_kib = None;

    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let destination = match key {
            "MemTotal" => &mut total_kib,
            "MemAvailable" => &mut available_kib,
            _ => continue,
        };
        if destination.is_some() {
            return Err(ParseError::DuplicateField {
                metric_source: LinuxMetricSource::ProcMeminfo,
                field: if key == "MemTotal" {
                    "MemTotal"
                } else {
                    "MemAvailable"
                },
            });
        }
        let mut tokens = value.split_ascii_whitespace();
        let amount = tokens
            .next()
            .ok_or_else(|| malformed(LinuxMetricSource::ProcMeminfo, "memory value is missing"))?;
        if tokens.next() != Some("kB") || tokens.next().is_some() {
            return Err(malformed(
                LinuxMetricSource::ProcMeminfo,
                "memory value must use the kB unit",
            ));
        }
        *destination = Some(parse_u64(LinuxMetricSource::ProcMeminfo, amount)?);
    }

    let total_kib = total_kib.ok_or(ParseError::MissingField {
        metric_source: LinuxMetricSource::ProcMeminfo,
        field: "MemTotal",
    })?;
    let available_kib = available_kib.ok_or(ParseError::MissingField {
        metric_source: LinuxMetricSource::ProcMeminfo,
        field: "MemAvailable",
    })?;
    if total_kib == 0 || available_kib > total_kib {
        return Err(malformed(
            LinuxMetricSource::ProcMeminfo,
            "memory totals are inconsistent",
        ));
    }
    let total_bytes = kib_to_bytes(total_kib)?;
    let available_bytes = kib_to_bytes(available_kib)?;
    let used_bytes = total_bytes
        .checked_sub(available_bytes)
        .ok_or(ParseError::ArithmeticOverflow)?;
    Ok(MemoryUsage {
        used_bytes,
        available_bytes,
        total_bytes,
    })
}

fn parse_net_dev(input: &[u8]) -> Result<BTreeMap<String, InterfaceCounters>, ParseError> {
    let text = bounded_text(
        LinuxMetricSource::ProcNetDev,
        input,
        MAX_NET_DEV_BYTES,
        MAX_NET_DEV_LINES,
    )?;
    let mut interfaces = BTreeMap::new();
    let mut header_lines = 0_usize;
    let mut data_lines = 0_usize;

    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Some((name, counters)) = line.split_once(':') else {
            if data_lines > 0 || header_lines == 2 {
                return Err(malformed(
                    LinuxMetricSource::ProcNetDev,
                    "unexpected network header or data line",
                ));
            }
            header_lines += 1;
            continue;
        };
        if header_lines != 2 {
            return Err(malformed(
                LinuxMetricSource::ProcNetDev,
                "expected two network header lines",
            ));
        }
        data_lines += 1;
        let name = name.trim();
        if name.is_empty()
            || name.len() > MAX_INTERFACE_NAME_BYTES
            || !name.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(malformed(
                LinuxMetricSource::ProcNetDev,
                "network interface name is invalid",
            ));
        }
        let values = counters
            .split_ascii_whitespace()
            .map(|token| parse_u64(LinuxMetricSource::ProcNetDev, token))
            .collect::<Result<Vec<_>, _>>()?;
        if values.len() != NETWORK_FIELD_COUNT {
            return Err(malformed(
                LinuxMetricSource::ProcNetDev,
                "network field count is invalid",
            ));
        }
        if name != "lo"
            && interfaces
                .insert(
                    name.to_owned(),
                    InterfaceCounters {
                        receive_bytes: values[0],
                        transmit_bytes: values[8],
                    },
                )
                .is_some()
        {
            return Err(ParseError::DuplicateField {
                metric_source: LinuxMetricSource::ProcNetDev,
                field: "interface",
            });
        }
    }
    if header_lines != 2 || data_lines == 0 {
        return Err(malformed(
            LinuxMetricSource::ProcNetDev,
            "network headers or interface rows are missing",
        ));
    }
    Ok(interfaces)
}

fn parse_root_df(input: &[u8]) -> Result<DiskUsage, ParseError> {
    let text = bounded_text(
        LinuxMetricSource::RootDisk,
        input,
        MAX_DF_BYTES,
        MAX_DF_LINES,
    )?;
    let lines = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if lines.len() != 2 {
        return Err(malformed(
            LinuxMetricSource::RootDisk,
            "expected one header and one root filesystem row",
        ));
    }
    let header = lines[0];
    if !header.contains("1024-blocks")
        || !header.contains("Available")
        || !header.contains("Mounted")
    {
        return Err(malformed(
            LinuxMetricSource::RootDisk,
            "df header does not match the version 1 contract",
        ));
    }

    let fields = lines[1].split_ascii_whitespace().collect::<Vec<_>>();
    if fields.len() < 6 || fields.len() > MAX_DF_FIELDS {
        return Err(malformed(
            LinuxMetricSource::RootDisk,
            "root filesystem field count is outside the provider contract",
        ));
    }
    let mount = fields[fields.len() - 1];
    let capacity = fields[fields.len() - 2];
    let available_kib = parse_u64(LinuxMetricSource::RootDisk, fields[fields.len() - 3])?;
    let reported_used_kib = parse_u64(LinuxMetricSource::RootDisk, fields[fields.len() - 4])?;
    let total_kib = parse_u64(LinuxMetricSource::RootDisk, fields[fields.len() - 5])?;
    let filesystem_id = fields[..fields.len() - 5].join(" ");

    if mount != "/" {
        return Err(malformed(
            LinuxMetricSource::RootDisk,
            "df row is not the controlled root mount",
        ));
    }
    if filesystem_id.is_empty() || filesystem_id.len() > MAX_FILESYSTEM_ID_BYTES {
        return Err(malformed(
            LinuxMetricSource::RootDisk,
            "filesystem identifier is invalid",
        ));
    }
    let capacity = capacity
        .strip_suffix('%')
        .ok_or_else(|| malformed(LinuxMetricSource::RootDisk, "capacity is invalid"))?;
    if parse_u64(LinuxMetricSource::RootDisk, capacity)? > 100 {
        return Err(malformed(
            LinuxMetricSource::RootDisk,
            "capacity is outside the supported range",
        ));
    }
    if total_kib == 0 || available_kib > total_kib || reported_used_kib > total_kib {
        return Err(malformed(
            LinuxMetricSource::RootDisk,
            "filesystem totals are inconsistent",
        ));
    }

    let total_bytes = kib_to_bytes(total_kib)?;
    let available_bytes = kib_to_bytes(available_kib)?;
    let used_bytes = total_bytes
        .checked_sub(available_bytes)
        .ok_or(ParseError::ArithmeticOverflow)?;
    Ok(DiskUsage {
        filesystem_id,
        mount: mount.to_owned(),
        used_bytes,
        available_bytes,
        total_bytes,
    })
}

fn calculate_cpu(
    previous: &CpuCounters,
    current: &CpuCounters,
) -> Result<MetricValue<CpuUsage>, ParseError> {
    if current
        .fields
        .iter()
        .zip(previous.fields.iter())
        .any(|(current, previous)| current < previous)
    {
        return Ok(MetricValue::Unavailable(
            MetricUnavailableReason::CounterReset,
        ));
    }
    let deltas = current
        .fields
        .iter()
        .zip(previous.fields.iter())
        .map(|(current, previous)| current - previous)
        .collect::<Vec<_>>();
    let total_delta = checked_sum(LinuxMetricSource::ProcStat, &deltas)?;
    if total_delta == 0 {
        return Ok(MetricValue::Unavailable(
            MetricUnavailableReason::NoCounterProgress,
        ));
    }
    let idle_delta = deltas[3]
        .checked_add(deltas[4])
        .ok_or(ParseError::ArithmeticOverflow)?;
    let active_delta = total_delta
        .checked_sub(idle_delta)
        .ok_or(ParseError::ArithmeticOverflow)?;
    let basis_points = u16::try_from(
        u128::from(active_delta)
            .checked_mul(10_000)
            .ok_or(ParseError::ArithmeticOverflow)?
            / u128::from(total_delta),
    )
    .map_err(|_| ParseError::ArithmeticOverflow)?;
    Ok(MetricValue::Available(CpuUsage { basis_points }))
}

fn calculate_network(
    previous: &BTreeMap<String, InterfaceCounters>,
    current: &BTreeMap<String, InterfaceCounters>,
    elapsed: Duration,
) -> Result<MetricValue<NetworkRate>, ParseError> {
    if previous.keys().ne(current.keys()) {
        return Ok(MetricValue::Unavailable(
            MetricUnavailableReason::CounterSetChanged,
        ));
    }
    let mut receive_delta = 0_u64;
    let mut transmit_delta = 0_u64;
    for (name, current) in current {
        let previous = previous.get(name).ok_or(ParseError::ArithmeticOverflow)?;
        let Some(interface_receive_delta) =
            current.receive_bytes.checked_sub(previous.receive_bytes)
        else {
            return Ok(MetricValue::Unavailable(
                MetricUnavailableReason::CounterReset,
            ));
        };
        let Some(interface_transmit_delta) =
            current.transmit_bytes.checked_sub(previous.transmit_bytes)
        else {
            return Ok(MetricValue::Unavailable(
                MetricUnavailableReason::CounterReset,
            ));
        };
        receive_delta = receive_delta
            .checked_add(interface_receive_delta)
            .ok_or(ParseError::ArithmeticOverflow)?;
        transmit_delta = transmit_delta
            .checked_add(interface_transmit_delta)
            .ok_or(ParseError::ArithmeticOverflow)?;
    }

    Ok(MetricValue::Available(NetworkRate {
        receive_bytes_per_second: rate_per_second(receive_delta, elapsed)?,
        transmit_bytes_per_second: rate_per_second(transmit_delta, elapsed)?,
    }))
}

fn rate_per_second(delta: u64, elapsed: Duration) -> Result<u64, ParseError> {
    let elapsed_nanos = elapsed.as_nanos();
    if elapsed_nanos == 0 {
        return Err(ParseError::NonMonotonicSampleTime);
    }
    let rate = u128::from(delta)
        .checked_mul(1_000_000_000)
        .ok_or(ParseError::ArithmeticOverflow)?
        / elapsed_nanos;
    u64::try_from(rate).map_err(|_| ParseError::ArithmeticOverflow)
}

fn bounded_text(
    source: LinuxMetricSource,
    input: &[u8],
    max_bytes: usize,
    max_lines: usize,
) -> Result<&str, ParseError> {
    if input.len() > max_bytes {
        return Err(ParseError::InputTooLarge {
            metric_source: source,
            max_bytes,
        });
    }
    let text = str::from_utf8(input).map_err(|_| ParseError::InvalidUtf8(source))?;
    if text.lines().count() > max_lines {
        return Err(ParseError::TooManyLines {
            metric_source: source,
            max_lines,
        });
    }
    if text
        .bytes()
        .any(|byte| byte.is_ascii_control() && !matches!(byte, b'\n' | b'\r' | b'\t'))
    {
        return Err(malformed(source, "output contains a control character"));
    }
    Ok(text)
}

fn parse_u64(source: LinuxMetricSource, token: &str) -> Result<u64, ParseError> {
    if token.is_empty() || !token.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ParseError::InvalidNumber {
            metric_source: source,
        });
    }
    token.parse::<u64>().map_err(|_| ParseError::InvalidNumber {
        metric_source: source,
    })
}

fn checked_sum(source: LinuxMetricSource, values: &[u64]) -> Result<u64, ParseError> {
    values.iter().try_fold(0_u64, |sum, value| {
        sum.checked_add(*value).ok_or(ParseError::InvalidNumber {
            metric_source: source,
        })
    })
}

fn kib_to_bytes(kib: u64) -> Result<u64, ParseError> {
    kib.checked_mul(1024).ok_or(ParseError::ArithmeticOverflow)
}

const fn malformed(source: LinuxMetricSource, detail: &'static str) -> ParseError {
    ParseError::Malformed {
        metric_source: source,
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEMINFO: &[u8] = b"MemTotal:       1000 kB\nMemFree:         100 kB\nMemAvailable:    800 kB\nHugePages_Total: 0\n";
    const ROOT_DF: &[u8] =
        b"Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/root 1000 250 750 25% /\n";

    fn stat(values: [u64; 8]) -> Vec<u8> {
        format!(
            "cpu {} {} {} {} {} {} {} {} 0 0\n",
            values[0], values[1], values[2], values[3], values[4], values[5], values[6], values[7]
        )
        .into_bytes()
    }

    fn net(interfaces: &[(&str, u64, u64)]) -> Vec<u8> {
        let mut output = String::from(
            "Inter-|   Receive                                                |  Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n",
        );
        for (name, receive, transmit) in interfaces {
            output.push_str(&format!(
                "{name}: {receive} 0 0 0 0 0 0 0 {transmit} 0 0 0 0 0 0 0\n"
            ));
        }
        output.into_bytes()
    }

    fn raw<'a>(proc_stat: &'a [u8], proc_net_dev: &'a [u8]) -> LinuxRawSample<'a> {
        LinuxRawSample {
            proc_stat,
            proc_meminfo: MEMINFO,
            proc_net_dev,
            root_df: ROOT_DF,
        }
    }

    #[test]
    fn collection_plan_is_fixed_versioned_bounded_and_uses_one_framed_probe() {
        assert_eq!(LINUX_PROVIDER_ID, "linux-procfs");
        assert_eq!(LINUX_PROVIDER_VERSION, 1);
        let command = str::from_utf8(LINUX_PROBE_COMMAND).unwrap();
        assert!(!command.contains("sudo"));
        assert!(!command.contains(['$', '`', '{', '}']));
        assert_eq!(command.matches("printf '\\000'").count(), 4);
        assert!(command.contains("head -n 1 /proc/stat"));
        assert!(command.contains("cat /proc/meminfo"));
        assert!(command.contains("cat /proc/net/dev"));
        assert!(command.contains("df -Pk -- /"));
        assert_eq!(
            LINUX_CPU_FOLLOW_UP_COMMAND,
            b"LC_ALL=C head -n 1 /proc/stat"
        );
    }

    #[test]
    fn parses_cpu_and_rejects_negative_overflow_or_extra_lines() {
        assert_eq!(
            parse_proc_stat(&stat([1, 2, 3, 4, 5, 6, 7, 8]))
                .unwrap()
                .fields,
            [1, 2, 3, 4, 5, 6, 7, 8]
        );
        assert!(parse_proc_stat(b"cpu 1 2 3 -4 5 6 7 8\n").is_err());
        assert!(parse_proc_stat(b"cpu 18446744073709551615 1 0 0 0 0 0 0\n").is_err());
        assert!(parse_proc_stat(b"cpu 1 2 3 4 5 6 7 8\ncpu0 1 2 3 4 5 6 7 8\n").is_err());
    }

    #[test]
    fn memory_uses_mem_available_and_preserves_a_real_zero_used_value() {
        let memory = parse_meminfo(b"MemTotal: 1024 kB\nMemAvailable: 1024 kB\n").unwrap();
        assert_eq!(memory.used_bytes, 0);
        assert_eq!(memory.available_bytes, 1024 * 1024);
        assert_eq!(memory.total_bytes, 1024 * 1024);
        assert!(parse_meminfo(b"MemTotal: 1024 kB\nMemAvailable: -1 kB\n").is_err());
        assert!(parse_meminfo(b"MemTotal: 10 kB\nMemAvailable: 11 kB\n").is_err());
        assert!(parse_meminfo(b"MemTotal: 10 MB\nMemAvailable: 5 kB\n").is_err());
    }

    #[test]
    fn network_parser_excludes_loopback_and_rejects_bad_rows() {
        let parsed = parse_net_dev(&net(&[("lo", 900, 800), ("eth0", 100, 200)])).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed["eth0"].receive_bytes, 100);
        let malformed = b"Inter-| Receive | Transmit\n face |bytes|bytes\neth0: -1 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n";
        assert!(parse_net_dev(malformed).is_err());
    }

    #[test]
    fn network_delta_overflow_fails_without_advancing_the_baseline() {
        let first_stat = stat([1, 0, 1, 8, 0, 0, 0, 0]);
        let second_stat = stat([2, 0, 2, 16, 0, 0, 0, 0]);
        let third_stat = stat([3, 0, 3, 24, 0, 0, 0, 0]);
        let first_net = net(&[("eth0", 0, 0), ("eth1", 0, 0)]);
        let overflowing_net = net(&[("eth0", u64::MAX, 1), ("eth1", 1, 1)]);
        let valid_net = net(&[("eth0", 2_000, 2_000), ("eth1", 2_000, 2_000)]);
        let mut provider = LinuxMetricsProvider::new();
        provider
            .sample(Duration::from_secs(1), raw(&first_stat, &first_net))
            .unwrap();
        assert_eq!(
            provider.sample(Duration::from_secs(2), raw(&second_stat, &overflowing_net)),
            Err(ParseError::ArithmeticOverflow)
        );

        let recovered = provider
            .sample(Duration::from_secs(3), raw(&third_stat, &valid_net))
            .unwrap();
        assert_eq!(
            recovered.network,
            MetricValue::Available(NetworkRate {
                receive_bytes_per_second: 2_000,
                transmit_bytes_per_second: 2_000,
            })
        );
    }

    #[test]
    fn root_df_uses_total_minus_available_and_rejects_uncontrolled_mounts() {
        let disk = parse_root_df(ROOT_DF).unwrap();
        assert_eq!(disk.filesystem_id, "/dev/root");
        assert_eq!(disk.mount, "/");
        assert_eq!(disk.used_bytes, 250 * 1024);
        assert_eq!(disk.available_bytes, 750 * 1024);
        assert!(parse_root_df(b"Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/data 1000 250 750 25% /data\n").is_err());
        assert!(parse_root_df(b"Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/root 1000 -1 750 25% /\n").is_err());
    }

    #[test]
    fn first_sample_builds_baselines_then_uses_cpu_and_monotonic_network_deltas() {
        let first_stat = stat([100, 0, 100, 800, 0, 0, 0, 0]);
        let second_stat = stat([150, 0, 150, 900, 0, 0, 0, 0]);
        let first_net = net(&[("lo", 5_000, 5_000), ("eth0", 1_000, 2_000)]);
        let second_net = net(&[("lo", 9_000, 9_000), ("eth0", 3_000, 3_000)]);
        let mut provider = LinuxMetricsProvider::new();

        let first = provider
            .sample(Duration::from_secs(10), raw(&first_stat, &first_net))
            .unwrap();
        assert_eq!(
            first.cpu,
            MetricValue::Unavailable(MetricUnavailableReason::InitialBaseline)
        );
        assert_eq!(
            first.network,
            MetricValue::Unavailable(MetricUnavailableReason::InitialBaseline)
        );

        let second = provider
            .sample(Duration::from_secs(12), raw(&second_stat, &second_net))
            .unwrap();
        assert_eq!(
            second.cpu,
            MetricValue::Available(CpuUsage {
                basis_points: 5_000
            })
        );
        assert_eq!(
            second.network,
            MetricValue::Available(NetworkRate {
                receive_bytes_per_second: 1_000,
                transmit_bytes_per_second: 500,
            })
        );
    }

    #[test]
    fn first_sample_cpu_can_be_completed_without_advancing_the_network_baseline() {
        let first_stat = stat([100, 0, 100, 800, 0, 0, 0, 0]);
        let follow_up_stat = stat([125, 0, 125, 850, 0, 0, 0, 0]);
        let next_stat = stat([150, 0, 150, 900, 0, 0, 0, 0]);
        let first_net = net(&[("eth0", 1_000, 2_000)]);
        let next_net = net(&[("eth0", 3_000, 3_000)]);
        let mut provider = LinuxMetricsProvider::new();

        let mut first = provider
            .sample(Duration::from_secs(10), raw(&first_stat, &first_net))
            .unwrap();
        first.cpu = provider.sample_cpu_follow_up(&follow_up_stat).unwrap();
        assert_eq!(
            first.cpu,
            MetricValue::Available(CpuUsage {
                basis_points: 5_000
            })
        );
        assert_eq!(
            first.network,
            MetricValue::Unavailable(MetricUnavailableReason::InitialBaseline)
        );

        let next = provider
            .sample(Duration::from_secs(12), raw(&next_stat, &next_net))
            .unwrap();
        assert_eq!(
            next.cpu,
            MetricValue::Available(CpuUsage {
                basis_points: 5_000
            })
        );
        assert_eq!(
            next.network,
            MetricValue::Available(NetworkRate {
                receive_bytes_per_second: 1_000,
                transmit_bytes_per_second: 500,
            })
        );
    }

    #[test]
    fn reports_real_zero_cpu_and_network_values_as_available() {
        let first_stat = stat([100, 0, 100, 800, 0, 0, 0, 0]);
        let second_stat = stat([100, 0, 100, 1_000, 0, 0, 0, 0]);
        let unchanged_net = net(&[("eth0", 100, 200)]);
        let mut provider = LinuxMetricsProvider::new();
        provider
            .sample(Duration::from_secs(1), raw(&first_stat, &unchanged_net))
            .unwrap();
        let sample = provider
            .sample(Duration::from_secs(2), raw(&second_stat, &unchanged_net))
            .unwrap();
        assert_eq!(
            sample.cpu,
            MetricValue::Available(CpuUsage { basis_points: 0 })
        );
        assert_eq!(
            sample.network,
            MetricValue::Available(NetworkRate {
                receive_bytes_per_second: 0,
                transmit_bytes_per_second: 0,
            })
        );
    }

    #[test]
    fn counter_resets_rebuild_baselines_and_recover_on_the_following_sample() {
        let start_stat = stat([100, 0, 100, 800, 0, 0, 0, 0]);
        let reset_stat = stat([10, 0, 10, 80, 0, 0, 0, 0]);
        let recovered_stat = stat([20, 0, 20, 160, 0, 0, 0, 0]);
        let start_net = net(&[("eth0", 1_000, 2_000)]);
        let reset_net = net(&[("eth0", 100, 200)]);
        let recovered_net = net(&[("eth0", 1_100, 2_200)]);
        let mut provider = LinuxMetricsProvider::new();
        provider
            .sample(Duration::from_secs(1), raw(&start_stat, &start_net))
            .unwrap();

        let reset = provider
            .sample(Duration::from_secs(2), raw(&reset_stat, &reset_net))
            .unwrap();
        assert_eq!(
            reset.cpu,
            MetricValue::Unavailable(MetricUnavailableReason::CounterReset)
        );
        assert_eq!(
            reset.network,
            MetricValue::Unavailable(MetricUnavailableReason::CounterReset)
        );

        let recovered = provider
            .sample(Duration::from_secs(3), raw(&recovered_stat, &recovered_net))
            .unwrap();
        assert!(recovered.cpu.is_available());
        assert_eq!(
            recovered.network,
            MetricValue::Available(NetworkRate {
                receive_bytes_per_second: 1_000,
                transmit_bytes_per_second: 2_000,
            })
        );
    }

    #[test]
    fn interface_set_changes_create_a_gap_before_the_new_baseline_is_used() {
        let first_stat = stat([1, 0, 1, 8, 0, 0, 0, 0]);
        let second_stat = stat([2, 0, 2, 16, 0, 0, 0, 0]);
        let first_net = net(&[("eth0", 100, 100)]);
        let second_net = net(&[("eth0", 200, 200), ("eth1", 50, 50)]);
        let mut provider = LinuxMetricsProvider::new();
        provider
            .sample(Duration::from_secs(1), raw(&first_stat, &first_net))
            .unwrap();
        let sample = provider
            .sample(Duration::from_secs(2), raw(&second_stat, &second_net))
            .unwrap();
        assert_eq!(
            sample.network,
            MetricValue::Unavailable(MetricUnavailableReason::CounterSetChanged)
        );
    }

    #[test]
    fn malformed_samples_and_non_monotonic_time_do_not_replace_the_baseline() {
        let first_stat = stat([100, 0, 100, 800, 0, 0, 0, 0]);
        let second_stat = stat([150, 0, 150, 900, 0, 0, 0, 0]);
        let first_net = net(&[("eth0", 1_000, 2_000)]);
        let second_net = net(&[("eth0", 3_000, 4_000)]);
        let mut provider = LinuxMetricsProvider::new();
        provider
            .sample(Duration::from_secs(10), raw(&first_stat, &first_net))
            .unwrap();

        let malformed_raw = LinuxRawSample {
            proc_stat: &second_stat,
            proc_meminfo: b"MemTotal: 1000 kB\n",
            proc_net_dev: &second_net,
            root_df: ROOT_DF,
        };
        assert!(
            provider
                .sample(Duration::from_secs(11), malformed_raw)
                .is_err()
        );
        assert_eq!(
            provider.sample(Duration::from_secs(10), raw(&second_stat, &second_net)),
            Err(ParseError::NonMonotonicSampleTime)
        );

        let sample = provider
            .sample(Duration::from_secs(12), raw(&second_stat, &second_net))
            .unwrap();
        assert_eq!(
            sample.network,
            MetricValue::Available(NetworkRate {
                receive_bytes_per_second: 1_000,
                transmit_bytes_per_second: 1_000,
            })
        );
    }

    #[test]
    fn all_sources_enforce_byte_and_line_bounds() {
        assert!(matches!(
            parse_proc_stat(&vec![b'0'; MAX_PROC_STAT_BYTES + 1]),
            Err(ParseError::InputTooLarge { .. })
        ));
        let too_many_meminfo_lines = "ignored: 0\n".repeat(MAX_MEMINFO_LINES + 1);
        assert!(matches!(
            parse_meminfo(too_many_meminfo_lines.as_bytes()),
            Err(ParseError::TooManyLines { .. })
        ));
        assert!(matches!(
            parse_net_dev(&[0xff]),
            Err(ParseError::InvalidUtf8(LinuxMetricSource::ProcNetDev))
        ));
        assert!(parse_root_df(b"Filesystem\0bad").is_err());
    }
}
