//! Real system readers, direct from `/proc`. No external system-info
//! dependency, matching the same rationale as the broker's own
//! `procinfo.rs`: this is trusted, security-adjacent-enough code (it feeds
//! an anomaly-detection model whose output could eventually inform
//! automated action) that keeping it small and fully readable end to end
//! matters more than saving a few lines with a heavier crate.

use std::fs;
use std::time::Duration;

pub fn read_uptime_seconds() -> f64 {
    fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(str::to_string))
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0)
}

/// Aggregate CPU utilization percent, sampled over `window`.
pub fn read_cpu_utilization_pct(window: Duration) -> f64 {
    let before = read_aggregate_cpu_line();
    std::thread::sleep(window);
    let after = read_aggregate_cpu_line();
    match (before, after) {
        (Some((busy_b, total_b)), Some((busy_a, total_a))) => {
            let busy_delta = busy_a.saturating_sub(busy_b) as f64;
            let total_delta = total_a.saturating_sub(total_b) as f64;
            if total_delta > 0.0 {
                (busy_delta / total_delta) * 100.0
            } else {
                0.0
            }
        }
        _ => 0.0,
    }
}

fn read_aggregate_cpu_line() -> Option<(u64, u64)> {
    let content = fs::read_to_string("/proc/stat").ok()?;
    let line = content.lines().next()?; // first line is the aggregate "cpu" line
    let fields: Vec<u64> = line.split_whitespace().skip(1).filter_map(|f| f.parse().ok()).collect();
    if fields.len() < 4 {
        return None;
    }
    let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
    let total: u64 = fields.iter().sum();
    Some((total.saturating_sub(idle), total))
}

pub struct MemInfo {
    pub used_pct: f64,
    pub available_mb: f64,
}

pub fn read_memory() -> MemInfo {
    let mut total_kb = 0u64;
    let mut available_kb = 0u64;
    if let Ok(content) = fs::read_to_string("/proc/meminfo") {
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                total_kb = parse_kb(rest);
            } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
                available_kb = parse_kb(rest);
            }
        }
    }
    let used_pct = if total_kb > 0 {
        ((total_kb.saturating_sub(available_kb)) as f64 / total_kb as f64) * 100.0
    } else {
        0.0
    };
    MemInfo {
        used_pct,
        available_mb: available_kb as f64 / 1024.0,
    }
}

fn parse_kb(rest: &str) -> u64 {
    rest.trim().trim_end_matches(" kB").parse().unwrap_or(0)
}

pub fn read_process_count() -> u64 {
    fs::read_dir("/proc")
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_name().to_str().is_some_and(|n| n.chars().all(|c| c.is_ascii_digit())))
                .count() as u64
        })
        .unwrap_or(0)
}

pub struct DiskIoRate {
    pub read_kbps: f64,
    pub write_kbps: f64,
}

/// Aggregate disk read/write throughput across all block devices listed in
/// `/proc/diskstats`, sampled over `window`. Units: KB/s (kibibytes),
/// derived from the 512-byte sector counts `/proc/diskstats` reports —
/// documented here since the schema's field name ("kbps") does not itself
/// specify kilobits vs kilobytes; this implementation treats every *kbps
/// field in this schema as kilobytes/sec for consistency, matching the
/// far more common convention for disk/network throughput in system
/// monitoring tools (iostat, etc.).
pub fn read_disk_io_rate(window: Duration) -> DiskIoRate {
    let before = read_diskstats_totals();
    std::thread::sleep(window);
    let after = read_diskstats_totals();

    let secs = window.as_secs_f64().max(0.001);
    let read_sectors = after.0.saturating_sub(before.0) as f64;
    let write_sectors = after.1.saturating_sub(before.1) as f64;
    const SECTOR_BYTES: f64 = 512.0;

    DiskIoRate {
        read_kbps: (read_sectors * SECTOR_BYTES / 1024.0) / secs,
        write_kbps: (write_sectors * SECTOR_BYTES / 1024.0) / secs,
    }
}

/// Sums sectors-read (field 6) and sectors-written (field 10) across every
/// real disk device line (skips partitions like `sda1` to avoid
/// double-counting the same I/O against both the whole disk and its
/// partitions).
fn read_diskstats_totals() -> (u64, u64) {
    let mut read_total = 0u64;
    let mut write_total = 0u64;
    if let Ok(content) = fs::read_to_string("/proc/diskstats") {
        for line in content.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 10 {
                continue;
            }
            let device = fields[2];
            if is_partition_or_virtual(device) {
                continue;
            }
            if let Ok(sectors_read) = fields[5].parse::<u64>() {
                read_total += sectors_read;
            }
            if let Ok(sectors_written) = fields[9].parse::<u64>() {
                write_total += sectors_written;
            }
        }
    }
    (read_total, write_total)
}

fn is_partition_or_virtual(device: &str) -> bool {
    // Heuristic: a device name ending in a digit after a letter-only prefix
    // (sda1, nvme0n1p1, mmcblk0p1) is a partition of a device counted
    // separately; loop/ram devices are virtual, not real disk I/O.
    device.starts_with("loop")
        || device.starts_with("ram")
        || device
            .chars()
            .last()
            .is_some_and(|c| c.is_ascii_digit() && device.contains(|c: char| c.is_ascii_alphabetic()))
            && (device.contains("p") || device.len() > 3)
}

pub struct NetIoRate {
    pub in_kbps: f64,
    pub out_kbps: f64,
}

/// Aggregate network throughput across all non-loopback interfaces in
/// `/proc/net/dev`, sampled over `window`. Same KB/s (kilobytes) unit
/// convention as disk I/O above, for consistency within this schema.
pub fn read_network_io_rate(window: Duration) -> NetIoRate {
    let before = read_net_dev_totals();
    std::thread::sleep(window);
    let after = read_net_dev_totals();

    let secs = window.as_secs_f64().max(0.001);
    let rx_bytes = after.0.saturating_sub(before.0) as f64;
    let tx_bytes = after.1.saturating_sub(before.1) as f64;

    NetIoRate {
        in_kbps: (rx_bytes / 1024.0) / secs,
        out_kbps: (tx_bytes / 1024.0) / secs,
    }
}

fn read_net_dev_totals() -> (u64, u64) {
    let mut rx_total = 0u64;
    let mut tx_total = 0u64;
    if let Ok(content) = fs::read_to_string("/proc/net/dev") {
        for line in content.lines().skip(2) {
            // first 2 lines are headers
            let Some((iface, rest)) = line.split_once(':') else {
                continue;
            };
            if iface.trim() == "lo" {
                continue;
            }
            let fields: Vec<&str> = rest.split_whitespace().collect();
            if fields.len() < 9 {
                continue;
            }
            if let Ok(rx) = fields[0].parse::<u64>() {
                rx_total += rx;
            }
            if let Ok(tx) = fields[8].parse::<u64>() {
                tx_total += tx;
            }
        }
    }
    (rx_total, tx_total)
}

/// Best-effort recent error/warning count from the kernel ring buffer
/// (`/proc/kmsg` requires root and is destructive to read, so this uses
/// `dmesg --level=err,warn` via a fixed, argument-free invocation instead
/// -- no user input reaches this command, so it is not the "AI-generated
/// shell command" pattern the broker's security model forbids elsewhere;
/// it is a fixed diagnostic call this binary always makes, identical to
/// how `df`/`ps` are commonly shelled out to by monitoring tools).
///
/// Returns 0 if `dmesg` is unavailable or unreadable without privilege --
/// per system_interface/docs/INTEGRATION.md's own guidance, an
/// unavailable metric is surfaced as its safe default rather than failing
/// the whole snapshot, and this limitation is documented in
/// telemetry-provider/README.md rather than silently assumed to mean
/// "zero errors occurred."
pub fn read_recent_error_event_count() -> u64 {
    use std::process::Command;
    let output = Command::new("dmesg").arg("--level=err,warn").arg("--since=-2min").output();
    match output {
        Ok(out) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout).lines().count() as u64
        }
        _ => 0,
    }
}
