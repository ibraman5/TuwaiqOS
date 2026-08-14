//! Cross-platform system readers, built on the `sysinfo` crate.

use std::time::Duration;

use sysinfo::{Networks, System};

pub const SAMPLE_WINDOW: Duration = Duration::from_millis(600);

pub struct Samples {
    pub cpu_utilization_pct: f64,
    pub ram_utilization_pct: f64,
    pub available_ram_mb: f64,
    pub process_count: u64,
    pub disk_read_kbps: f64,
    pub disk_write_kbps: f64,
    pub network_in_kbps: f64,
    pub network_out_kbps: f64,
    pub uptime_seconds: f64,
}

pub fn take_samples() -> Samples {
    let mut sys = System::new_all();
    let mut networks = Networks::new_with_refreshed_list();

    sys.refresh_cpu_usage();
    networks.refresh();

    std::thread::sleep(SAMPLE_WINDOW);

    sys.refresh_cpu_usage();
    sys.refresh_memory();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    networks.refresh();

    let cpu_utilization_pct = sys.global_cpu_usage() as f64;

    let total_kb = sys.total_memory();
    let available_kb = sys.available_memory();
    let ram_utilization_pct = if total_kb > 0 {
        ((total_kb.saturating_sub(available_kb)) as f64 / total_kb as f64) * 100.0
    } else {
        0.0
    };
    let available_ram_mb = available_kb as f64 / 1024.0 / 1024.0;

    let process_count = sys.processes().len() as u64;
    let uptime_seconds = System::uptime() as f64;

    let secs = SAMPLE_WINDOW.as_secs_f64().max(0.001);

    let (mut read_bytes, mut write_bytes) = (0u64, 0u64);
    for process in sys.processes().values() {
        let usage = process.disk_usage();
        read_bytes += usage.read_bytes;
        write_bytes += usage.written_bytes;
    }
    let disk_read_kbps = (read_bytes as f64 / 1024.0) / secs;
    let disk_write_kbps = (write_bytes as f64 / 1024.0) / secs;

    let (mut rx_bytes, mut tx_bytes) = (0u64, 0u64);
    for (name, data) in networks.iter() {
        if is_loopback_like(name) {
            continue;
        }
        rx_bytes += data.received();
        tx_bytes += data.transmitted();
    }
    let network_in_kbps = (rx_bytes as f64 / 1024.0) / secs;
    let network_out_kbps = (tx_bytes as f64 / 1024.0) / secs;

    Samples {
        cpu_utilization_pct,
        ram_utilization_pct,
        available_ram_mb,
        process_count,
        disk_read_kbps,
        disk_write_kbps,
        network_in_kbps,
        network_out_kbps,
        uptime_seconds,
    }
}

fn is_loopback_like(interface_name: &str) -> bool {
    let lower = interface_name.to_lowercase();
    lower == "lo" || lower.starts_with("loopback") || lower.contains("loopback")
}

pub fn read_recent_error_event_count() -> u64 {
    #[cfg(unix)]
    {
        use std::process::Command;
        let output = Command::new("dmesg")
            .arg("--level=err,warn")
            .arg("--since=-2min")
            .output();
        match output {
            Ok(out) if out.status.success() => {
                String::from_utf8_lossy(&out.stdout).lines().count() as u64
            }
            _ => 0,
        }
    }
    #[cfg(not(unix))]
    {
        0
    }
}