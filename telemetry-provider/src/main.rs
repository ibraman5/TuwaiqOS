//! tuwaiq-telemetry-provider -- Stage 1 read-only System Telemetry
//! Provider.

mod readers;
mod service_state;
#[cfg(test)]
mod service_state_tests;
mod snapshot;

use snapshot::TelemetrySnapshot;

fn main() {
    let timestamp = chrono::Utc::now().to_rfc3339();

    let samples = readers::take_samples();
    let error_event_count = readers::read_recent_error_event_count();

    let state = service_state::derive_service_state(
        samples.cpu_utilization_pct,
        samples.ram_utilization_pct,
        error_event_count,
    );

    let snapshot = TelemetrySnapshot {
        timestamp,
        cpu_utilization_pct: samples.cpu_utilization_pct,
        ram_utilization_pct: samples.ram_utilization_pct,
        available_ram_mb: samples.available_ram_mb,
        process_count: samples.process_count,
        disk_read_kbps: samples.disk_read_kbps,
        disk_write_kbps: samples.disk_write_kbps,
        network_in_kbps: samples.network_in_kbps,
        network_out_kbps: samples.network_out_kbps,
        uptime_seconds: samples.uptime_seconds,
        error_event_count,
        service_state: state,
        source: "future_real",
        scenario_label: "live_snapshot",
        is_synthetic_anomaly: false,
    };

    match serde_json::to_string_pretty(&snapshot) {
        Ok(json) => println!("{json}"),
        Err(e) => {
            eprintln!("tuwaiq-telemetry-provider: failed to serialize snapshot: {e}");
            std::process::exit(1);
        }
    }
}