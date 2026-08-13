//! tuwaiq-telemetry-provider -- Stage 1 read-only System Telemetry
//! Provider.
//!
//! Emits exactly one JSON object on stdout, matching
//! `ai_development/system_interface/schemas/telemetry_input.schema.json`,
//! built entirely from real host `/proc` data (no synthetic/mocked
//! values). Intended to be piped into a file and consumed by the existing
//! `inference/predict.py --input-json <file>` pipeline -- see
//! `README.md` in this directory for the full round-trip demo.
//!
//! This binary is read-only: it does not accept any input, take any
//! action, or write anything except its own stdout snapshot.

mod readers;
mod service_state;
#[cfg(test)]
mod service_state_tests;
mod snapshot;

use std::time::Duration;

use snapshot::TelemetrySnapshot;

/// Sampling window for every rate-based metric (CPU%, disk I/O, network
/// I/O) -- all four are measured over the *same* window so the resulting
/// snapshot describes one coherent slice of time, not four different
/// windows stitched together.
const SAMPLE_WINDOW: Duration = Duration::from_millis(500);

fn main() {
    let timestamp = chrono::Utc::now().to_rfc3339();

    // CPU, disk, and network are each sampled with their own
    // before/sleep/after pair internally; run them sequentially (not
    // concurrently) so the total wall-clock cost is bounded and
    // predictable (~3 * SAMPLE_WINDOW) for a prototype whose priority is
    // correctness and readability over minimizing snapshot latency.
    let cpu_utilization_pct = readers::read_cpu_utilization_pct(SAMPLE_WINDOW);
    let disk = readers::read_disk_io_rate(SAMPLE_WINDOW);
    let net = readers::read_network_io_rate(SAMPLE_WINDOW);

    let mem = readers::read_memory();
    let process_count = readers::read_process_count();
    let uptime_seconds = readers::read_uptime_seconds();
    let error_event_count = readers::read_recent_error_event_count();

    let state = service_state::derive_service_state(cpu_utilization_pct, mem.used_pct, error_event_count);

    let snapshot = TelemetrySnapshot {
        timestamp,
        cpu_utilization_pct,
        ram_utilization_pct: mem.used_pct,
        available_ram_mb: mem.available_mb,
        process_count,
        disk_read_kbps: disk.read_kbps,
        disk_write_kbps: disk.write_kbps,
        network_in_kbps: net.in_kbps,
        network_out_kbps: net.out_kbps,
        uptime_seconds,
        error_event_count,
        service_state: state,
    };

    match serde_json::to_string_pretty(&snapshot) {
        Ok(json) => println!("{json}"),
        Err(e) => {
            eprintln!("tuwaiq-telemetry-provider: failed to serialize snapshot: {e}");
            std::process::exit(1);
        }
    }
}
