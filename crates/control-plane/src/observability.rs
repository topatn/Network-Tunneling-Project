//! Control-plane observability, Prometheus exposition, and redacted structured logging
//! (`T-103`, `OPS-002`, `OPS-006`, `SEC-013`, `PRIV-002`, `architecture.md` §15).
//!
//! Exposes Prometheus text-format metrics for control plane operations:
//! - Active tunnels and allocation lifecycle counters (`OPS-002`)
//! - Per-relay health status, heartbeat staleness, and port pool saturation (`REL-002`, `OPS-002`)
//! - Relay egress bytes recorded in usage accounting (`OPS-006`)
//! - Rate-limiting enforcement counters by class (`contracts.md` §5.1)
//!
//! Provides structured JSON-lines logging with strict automated redaction of tokens,
//! secrets, raw IP addresses, and payload bytes (`PRIV-002`, `INV-5`).

use serde_json::json;
use std::fmt::Write;

use crate::redaction::{redact_json_value, scan_for_sensitive_leaks};

/// Telemetry metrics for an individual relay node tracked by the control plane (`REL-002`, `OPS-002`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayNodeTelemetry {
    /// Relay unique identifier.
    pub relay_id: String,
    /// Geographic region (e.g. `"staging-us-east"`).
    pub region: String,
    /// Administrative status (`"healthy"`, `"unhealthy"`, `"draining"`, `"retired"`).
    pub status: String,
    /// Unix timestamp (seconds) of the last received heartbeat report.
    pub last_heartbeat_epoch_s: i64,
    /// Currently allocated public ports on this relay node.
    pub ports_allocated: u16,
    /// Total configured port capacity on this relay node (`port_range_end - port_range_start + 1`).
    pub ports_capacity: u16,
    /// Cumulative outbound egress bytes recorded for this relay node from usage reports (`OPS-006`).
    pub egress_bytes_recorded: u64,
}

/// Point-in-time telemetry snapshot of the control plane (`OPS-002`, `OPS-006`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlPlaneTelemetrySnapshot {
    /// Environment name (`"staging"`, `"production"`, `"dev"`).
    pub environment: String,
    /// Service version string.
    pub version: String,
    /// Process uptime in seconds.
    pub uptime_secs: u64,
    /// Currently active (non-terminal) tunnels.
    pub active_tunnels: u64,
    /// Total tunnel allocation requests received.
    pub allocation_requests_total: u64,
    /// Total tunnel allocation failures across all causes.
    pub allocation_failures_total: u64,
    /// Allocation failures broken down by specific reason.
    pub allocation_failures_by_reason: Vec<(String, u64)>,
    /// Rate limit rejections broken down by class (`strict`, `normal`, `lenient`).
    pub rate_limit_rejections: Vec<(String, u64)>,
    /// Per-relay operational status and saturation metrics.
    pub relays: Vec<RelayNodeTelemetry>,
}

/// Format a control-plane telemetry snapshot into the standard Prometheus exposition format.
pub fn format_control_plane_prometheus(snap: &ControlPlaneTelemetrySnapshot) -> String {
    let mut out = String::with_capacity(8192);

    let env = escape_label_value(&snap.environment);
    let ver = escape_label_value(&snap.version);
    let base_labels = format!("environment=\"{env}\"");

    // 1. Process info & uptime
    let _ = writeln!(
        out,
        "# HELP gtt_control_plane_info Information about the running control plane binary."
    );
    let _ = writeln!(out, "# TYPE gtt_control_plane_info gauge");
    let _ = writeln!(
        out,
        "gtt_control_plane_info{{{base_labels},version=\"{ver}\"}} 1"
    );

    let _ = writeln!(
        out,
        "# HELP gtt_control_plane_uptime_seconds Seconds since control plane start."
    );
    let _ = writeln!(out, "# TYPE gtt_control_plane_uptime_seconds gauge");
    let _ = writeln!(
        out,
        "gtt_control_plane_uptime_seconds{{{base_labels}}} {}",
        snap.uptime_secs
    );

    // 2. Active Tunnels
    let _ = writeln!(
        out,
        "# HELP gtt_control_plane_tunnels_active Number of currently active tunnels in PostgreSQL."
    );
    let _ = writeln!(out, "# TYPE gtt_control_plane_tunnels_active gauge");
    let _ = writeln!(
        out,
        "gtt_control_plane_tunnels_active{{{base_labels}}} {}",
        snap.active_tunnels
    );

    // 3. Allocation Requests & Failure Spikes (OPS-002)
    let _ = writeln!(
        out,
        "# HELP gtt_control_plane_allocation_requests_total Total allocation requests received."
    );
    let _ = writeln!(
        out,
        "# TYPE gtt_control_plane_allocation_requests_total counter"
    );
    let _ = writeln!(
        out,
        "gtt_control_plane_allocation_requests_total{{{base_labels}}} {}",
        snap.allocation_requests_total
    );

    let _ = writeln!(out, "# HELP gtt_control_plane_allocation_failures_total Total failed allocation attempts by reason.");
    let _ = writeln!(
        out,
        "# TYPE gtt_control_plane_allocation_failures_total counter"
    );
    for (reason, count) in &snap.allocation_failures_by_reason {
        let r_esc = escape_label_value(reason);
        let _ = writeln!(out, "gtt_control_plane_allocation_failures_total{{{base_labels},reason=\"{r_esc}\"}} {count}");
    }
    if snap.allocation_failures_by_reason.is_empty() {
        let _ = writeln!(
            out,
            "gtt_control_plane_allocation_failures_total{{{base_labels},reason=\"all\"}} {}",
            snap.allocation_failures_total
        );
    }

    // 4. Rate Limiting Rejections
    let _ = writeln!(out, "# HELP gtt_control_plane_rate_limit_rejections_total Total 429 rate_limited rejections by class.");
    let _ = writeln!(
        out,
        "# TYPE gtt_control_plane_rate_limit_rejections_total counter"
    );
    for (class, count) in &snap.rate_limit_rejections {
        let c_esc = escape_label_value(class);
        let _ = writeln!(out, "gtt_control_plane_rate_limit_rejections_total{{{base_labels},class=\"{c_esc}\"}} {count}");
    }

    // 5. Per-Relay Health, Heartbeat & Port Pool Saturation (OPS-002, REL-002)
    let _ = writeln!(out, "# HELP gtt_control_plane_relay_heartbeat_timestamp_seconds Unix timestamp of last relay heartbeat.");
    let _ = writeln!(
        out,
        "# TYPE gtt_control_plane_relay_heartbeat_timestamp_seconds gauge"
    );
    for r in &snap.relays {
        let r_id = escape_label_value(&r.relay_id);
        let r_reg = escape_label_value(&r.region);
        let _ = writeln!(out, "gtt_control_plane_relay_heartbeat_timestamp_seconds{{{base_labels},relay_id=\"{r_id}\",region=\"{r_reg}\"}} {}", r.last_heartbeat_epoch_s);
    }

    let _ = writeln!(
        out,
        "# HELP gtt_control_plane_relay_status Current administrative status (1 if active)."
    );
    let _ = writeln!(out, "# TYPE gtt_control_plane_relay_status gauge");
    for r in &snap.relays {
        let r_id = escape_label_value(&r.relay_id);
        let r_reg = escape_label_value(&r.region);
        for status_val in &["healthy", "unhealthy", "draining", "retired"] {
            let is_match = if r.status.eq_ignore_ascii_case(status_val) {
                1
            } else {
                0
            };
            let _ = writeln!(out, "gtt_control_plane_relay_status{{{base_labels},relay_id=\"{r_id}\",region=\"{r_reg}\",status=\"{status_val}\"}} {is_match}");
        }
    }

    let _ = writeln!(
        out,
        "# HELP gtt_control_plane_relay_ports_allocated Number of public ports allocated on relay."
    );
    let _ = writeln!(out, "# TYPE gtt_control_plane_relay_ports_allocated gauge");
    for r in &snap.relays {
        let r_id = escape_label_value(&r.relay_id);
        let r_reg = escape_label_value(&r.region);
        let _ = writeln!(out, "gtt_control_plane_relay_ports_allocated{{{base_labels},relay_id=\"{r_id}\",region=\"{r_reg}\"}} {}", r.ports_allocated);
    }

    let _ = writeln!(out, "# HELP gtt_control_plane_relay_ports_capacity Total public port capacity configured on relay.");
    let _ = writeln!(out, "# TYPE gtt_control_plane_relay_ports_capacity gauge");
    for r in &snap.relays {
        let r_id = escape_label_value(&r.relay_id);
        let r_reg = escape_label_value(&r.region);
        let _ = writeln!(out, "gtt_control_plane_relay_ports_capacity{{{base_labels},relay_id=\"{r_id}\",region=\"{r_reg}\"}} {}", r.ports_capacity);
    }

    let _ = writeln!(out, "# HELP gtt_control_plane_relay_port_pool_utilization_ratio Ratio of allocated ports to total capacity.");
    let _ = writeln!(
        out,
        "# TYPE gtt_control_plane_relay_port_pool_utilization_ratio gauge"
    );
    for r in &snap.relays {
        let r_id = escape_label_value(&r.relay_id);
        let r_reg = escape_label_value(&r.region);
        let ratio = if r.ports_capacity > 0 {
            r.ports_allocated as f64 / r.ports_capacity as f64
        } else {
            0.0
        };
        let _ = writeln!(out, "gtt_control_plane_relay_port_pool_utilization_ratio{{{base_labels},relay_id=\"{r_id}\",region=\"{r_reg}\"}} {ratio:.4}");
    }

    // 6. Egress Bytes Recorded per Relay (OPS-006 Cost Monitoring)
    let _ = writeln!(out, "# HELP gtt_control_plane_relay_egress_bytes_recorded_total Cumulative egress bytes recorded via usage reports.");
    let _ = writeln!(
        out,
        "# TYPE gtt_control_plane_relay_egress_bytes_recorded_total counter"
    );
    for r in &snap.relays {
        let r_id = escape_label_value(&r.relay_id);
        let r_reg = escape_label_value(&r.region);
        let _ = writeln!(out, "gtt_control_plane_relay_egress_bytes_recorded_total{{{base_labels},relay_id=\"{r_id}\",region=\"{r_reg}\"}} {}", r.egress_bytes_recorded);
    }

    out
}

/// Build a redacted structured log entry formatted as JSON-lines (`PRIV-002`, `SEC-013`).
///
/// Automatically redacts sensitive fields and raw IP addresses before returning,
/// and verifies that `scan_for_sensitive_leaks` returns zero findings.
pub fn format_structured_log(
    timestamp_rfc3339: &str,
    level: &str,
    target: &str,
    event: &str,
    mut fields: serde_json::Value,
) -> String {
    redact_json_value(&mut fields);

    let log_obj = json!({
        "timestamp": timestamp_rfc3339,
        "level": level,
        "target": target,
        "event": event,
        "fields": fields,
    });

    let serialized = log_obj.to_string();

    // Redaction invariant check: never emit unredacted sensitive leaks
    let leaks = scan_for_sensitive_leaks(&serialized);
    if !leaks.is_empty() {
        // Defensive fallback: emit safe redacted error line
        let fallback = json!({
            "timestamp": timestamp_rfc3339,
            "level": "error",
            "target": "observability::logger",
            "event": "log_entry_dropped_sensitive_leak_detected",
            "fields": { "reasons": leaks },
        });
        return fallback.to_string();
    }

    serialized
}

fn escape_label_value(val: &str) -> String {
    let mut escaped = String::with_capacity(val.len());
    for c in val.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            other => escaped.push(other),
        }
    }
    escaped
}
