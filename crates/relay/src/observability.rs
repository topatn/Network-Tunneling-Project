//! Prometheus metrics exposition and telemetry snapshot formatting for relay nodes
//! (`T-103`, `OPS-002`, `OPS-004`, `OPS-006`, `architecture.md` §15).
//!
//! Provides a structured telemetry snapshot across relay subsystems (admission enforcement,
//! rate/backpressure limits, TCP forwarding, and UDP flow management) and formats them into
//! the standard Prometheus text-based exposition format for operational scraping and alerting.

use std::fmt::Write;

use crate::admissions::RelayAdmissionMetrics;
use crate::limits::RelayLimitMetrics;
use crate::tcp::RelayTcpMetrics;
use crate::udp::RelayUdpMetrics;

/// Point-in-time telemetry snapshot of a relay node (`OPS-002`, `OPS-004`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayTelemetrySnapshot {
    /// Unique identifier of this relay node.
    pub relay_id: String,
    /// Geographic region or datacenter (e.g., `"staging-us-east"`, `"prod-eu-central"`).
    pub region: String,
    /// Process uptime in seconds.
    pub uptime_secs: u64,
    /// Currently connected tunnel sessions from host engines.
    pub sessions_active: usize,
    /// Guest admission enforcement metrics.
    pub admissions: RelayAdmissionMetrics,
    /// Queue and bandwidth limit / backpressure metrics.
    pub limits: RelayLimitMetrics,
    /// TCP stream forwarding metrics.
    pub tcp: RelayTcpMetrics,
    /// UDP flow and datagram forwarding metrics.
    pub udp: RelayUdpMetrics,
    /// Current queued bytes across all sessions in global relay memory.
    pub global_queued_bytes: usize,
    /// Maximum configured global queue memory budget (e.g. 16 MiB).
    pub global_memory_budget_bytes: usize,
}

/// Format a relay telemetry snapshot into the standard Prometheus exposition format.
///
/// Bounded and allocation-conscious: pre-allocates an 8 KiB buffer and escapes any
/// label values safely per the Prometheus text-based format specification.
pub fn format_relay_prometheus(snap: &RelayTelemetrySnapshot) -> String {
    let mut out = String::with_capacity(8192);

    let id = escape_label_value(&snap.relay_id);
    let region = escape_label_value(&snap.region);
    let base_labels = format!("relay_id=\"{id}\",region=\"{region}\"");

    // 1. Process info & uptime
    let _ = writeln!(
        out,
        "# HELP gtt_relay_uptime_seconds Seconds since relay process start."
    );
    let _ = writeln!(out, "# TYPE gtt_relay_uptime_seconds gauge");
    let _ = writeln!(
        out,
        "gtt_relay_uptime_seconds{{{base_labels}}} {}",
        snap.uptime_secs
    );

    // 2. Active sessions
    let _ = writeln!(
        out,
        "# HELP gtt_relay_sessions_active Active tunnel sessions connected."
    );
    let _ = writeln!(out, "# TYPE gtt_relay_sessions_active gauge");
    let _ = writeln!(
        out,
        "gtt_relay_sessions_active{{{base_labels}}} {}",
        snap.sessions_active
    );

    // 3. TCP flows & volume
    let _ = writeln!(
        out,
        "# HELP gtt_relay_tcp_flows_active Currently active TCP forwarding flows."
    );
    let _ = writeln!(out, "# TYPE gtt_relay_tcp_flows_active gauge");
    let _ = writeln!(
        out,
        "gtt_relay_tcp_flows_active{{{base_labels}}} {}",
        snap.tcp.active_tcp_flows
    );

    let _ = writeln!(
        out,
        "# HELP gtt_relay_tcp_connections_total Total guest TCP connections opened."
    );
    let _ = writeln!(out, "# TYPE gtt_relay_tcp_connections_total counter");
    let _ = writeln!(
        out,
        "gtt_relay_tcp_connections_total{{{base_labels}}} {}",
        snap.tcp.total_connected
    );

    let _ = writeln!(
        out,
        "# HELP gtt_relay_tcp_refused_total Total guest TCP connections refused."
    );
    let _ = writeln!(out, "# TYPE gtt_relay_tcp_refused_total counter");
    let _ = writeln!(
        out,
        "gtt_relay_tcp_refused_total{{{base_labels}}} {}",
        snap.tcp.total_refused
    );

    let _ = writeln!(
        out,
        "# HELP gtt_relay_tcp_limit_reached_total Guest TCP connections refused due to flow caps."
    );
    let _ = writeln!(out, "# TYPE gtt_relay_tcp_limit_reached_total counter");
    let _ = writeln!(
        out,
        "gtt_relay_tcp_limit_reached_total{{{base_labels}}} {}",
        snap.tcp.total_limit_reached
    );

    // 4. UDP flows & volume
    let _ = writeln!(
        out,
        "# HELP gtt_relay_udp_flows_active Currently active UDP flows in the NAT table."
    );
    let _ = writeln!(out, "# TYPE gtt_relay_udp_flows_active gauge");
    let _ = writeln!(
        out,
        "gtt_relay_udp_flows_active{{{base_labels}}} {}",
        snap.udp.active_udp_flows
    );

    let _ = writeln!(out, "# HELP gtt_relay_udp_datagrams_forwarded_total Total datagrams forwarded between guest and engine.");
    let _ = writeln!(
        out,
        "# TYPE gtt_relay_udp_datagrams_forwarded_total counter"
    );
    let _ = writeln!(
        out,
        "gtt_relay_udp_datagrams_forwarded_total{{{base_labels},direction=\"ingress\"}} {}",
        snap.udp.datagrams_guest_to_engine
    );
    let _ = writeln!(
        out,
        "gtt_relay_udp_datagrams_forwarded_total{{{base_labels},direction=\"egress\"}} {}",
        snap.udp.datagrams_engine_to_guest
    );

    let _ = writeln!(
        out,
        "# HELP gtt_relay_udp_flows_expired_total Total UDP flows expired due to inactivity."
    );
    let _ = writeln!(out, "# TYPE gtt_relay_udp_flows_expired_total counter");
    let _ = writeln!(
        out,
        "gtt_relay_udp_flows_expired_total{{{base_labels}}} {}",
        snap.udp.flows_expired
    );

    // 5. Ingress & Egress Bytes (OPS-006 Cost Monitoring)
    let _ = writeln!(
        out,
        "# HELP gtt_relay_ingress_bytes_total Total inbound bytes received from guests/engine."
    );
    let _ = writeln!(out, "# TYPE gtt_relay_ingress_bytes_total counter");
    let _ = writeln!(
        out,
        "gtt_relay_ingress_bytes_total{{{base_labels},protocol=\"tcp\"}} {}",
        snap.tcp.bytes_guest_to_engine
    );

    let _ = writeln!(out, "# HELP gtt_relay_egress_bytes_total Total outbound bytes forwarded from relay (dominant hosting cost driver).");
    let _ = writeln!(out, "# TYPE gtt_relay_egress_bytes_total counter");
    let _ = writeln!(
        out,
        "gtt_relay_egress_bytes_total{{{base_labels},protocol=\"tcp\"}} {}",
        snap.tcp.bytes_engine_to_guest
    );

    // 6. Queue bounds, Memory & Backpressure (OPS-004)
    let _ = writeln!(
        out,
        "# HELP gtt_relay_frames_enqueued_total Total frames accepted into session queues."
    );
    let _ = writeln!(out, "# TYPE gtt_relay_frames_enqueued_total counter");
    let _ = writeln!(
        out,
        "gtt_relay_frames_enqueued_total{{{base_labels}}} {}",
        snap.limits.frames_enqueued
    );

    let _ = writeln!(
        out,
        "# HELP gtt_relay_bytes_enqueued_total Total bytes accepted into session queues."
    );
    let _ = writeln!(out, "# TYPE gtt_relay_bytes_enqueued_total counter");
    let _ = writeln!(
        out,
        "gtt_relay_bytes_enqueued_total{{{base_labels}}} {}",
        snap.limits.bytes_enqueued
    );

    let _ = writeln!(
        out,
        "# HELP gtt_relay_queued_frames Total frames currently waiting in session queues."
    );
    let _ = writeln!(out, "# TYPE gtt_relay_queued_frames gauge");
    let _ = writeln!(
        out,
        "gtt_relay_queued_frames{{{base_labels}}} {}",
        snap.limits.queued_frames
    );

    let _ = writeln!(
        out,
        "# HELP gtt_relay_queued_bytes Total bytes currently waiting in session queues."
    );
    let _ = writeln!(out, "# TYPE gtt_relay_queued_bytes gauge");
    let _ = writeln!(
        out,
        "gtt_relay_queued_bytes{{{base_labels}}} {}",
        snap.limits.queued_bytes
    );

    let _ = writeln!(out, "# HELP gtt_relay_global_queued_bytes Total bytes queued across all sessions on this relay.");
    let _ = writeln!(out, "# TYPE gtt_relay_global_queued_bytes gauge");
    let _ = writeln!(
        out,
        "gtt_relay_global_queued_bytes{{{base_labels}}} {}",
        snap.global_queued_bytes
    );

    let _ = writeln!(
        out,
        "# HELP gtt_relay_global_memory_budget_bytes Configured global memory ceiling for queues."
    );
    let _ = writeln!(out, "# TYPE gtt_relay_global_memory_budget_bytes gauge");
    let _ = writeln!(
        out,
        "gtt_relay_global_memory_budget_bytes{{{base_labels}}} {}",
        snap.global_memory_budget_bytes
    );

    let _ = writeln!(out, "# HELP gtt_relay_global_memory_utilization_ratio Ratio of global queued bytes to configured budget.");
    let _ = writeln!(
        out,
        "# TYPE gtt_relay_global_memory_utilization_ratio gauge"
    );
    let mem_ratio = if snap.global_memory_budget_bytes > 0 {
        snap.global_queued_bytes as f64 / snap.global_memory_budget_bytes as f64
    } else {
        0.0
    };
    let _ = writeln!(
        out,
        "gtt_relay_global_memory_utilization_ratio{{{base_labels}}} {mem_ratio:.4}"
    );

    // 7. Dropped frames & datagrams by reason (OPS-002 Saturation Alerting)
    let _ = writeln!(out, "# HELP gtt_relay_frames_dropped_total Total frames/datagrams dropped due to limits or backpressure.");
    let _ = writeln!(out, "# TYPE gtt_relay_frames_dropped_total counter");
    let _ = writeln!(
        out,
        "gtt_relay_frames_dropped_total{{{base_labels},reason=\"queue_frames_full\"}} {}",
        snap.limits.dropped_queue_frames_full
    );
    let _ = writeln!(
        out,
        "gtt_relay_frames_dropped_total{{{base_labels},reason=\"queue_bytes_full\"}} {}",
        snap.limits.dropped_queue_bytes_full
    );
    let _ = writeln!(
        out,
        "gtt_relay_frames_dropped_total{{{base_labels},reason=\"global_memory_cap\"}} {}",
        snap.limits.dropped_global_memory_cap
    );
    let _ = writeln!(
        out,
        "gtt_relay_frames_dropped_total{{{base_labels},reason=\"bandwidth_exceeded\"}} {}",
        snap.limits.dropped_bandwidth_exceeded
    );
    let _ = writeln!(
        out,
        "gtt_relay_frames_dropped_total{{{base_labels},reason=\"oversize\"}} {}",
        snap.udp.dropped_oversize
    );
    let _ = writeln!(
        out,
        "gtt_relay_frames_dropped_total{{{base_labels},reason=\"unknown_mapping\"}} {}",
        snap.udp.dropped_unknown_mapping
    );
    let _ = writeln!(
        out,
        "gtt_relay_frames_dropped_total{{{base_labels},reason=\"malformed\"}} {}",
        snap.udp.dropped_malformed
    );
    let _ = writeln!(
        out,
        "gtt_relay_frames_dropped_total{{{base_labels},reason=\"flow_limit\"}} {}",
        snap.udp.dropped_flow_limit
    );
    let _ = writeln!(
        out,
        "gtt_relay_frames_dropped_total{{{base_labels},reason=\"unknown_flow\"}} {}",
        snap.udp.dropped_unknown_flow
    );

    // 8. Guest Admissions (FR-009, SEC-007)
    let _ = writeln!(
        out,
        "# HELP gtt_relay_admissions_checks_total Total guest admission checks performed."
    );
    let _ = writeln!(out, "# TYPE gtt_relay_admissions_checks_total counter");
    let _ = writeln!(
        out,
        "gtt_relay_admissions_checks_total{{{base_labels},result=\"admitted\"}} {}",
        snap.admissions.admitted_total
    );
    let _ = writeln!(
        out,
        "gtt_relay_admissions_checks_total{{{base_labels},result=\"refused_unadmitted\"}} {}",
        snap.admissions.refused_unadmitted_total
    );
    let _ = writeln!(
        out,
        "gtt_relay_admissions_checks_total{{{base_labels},result=\"refused_expired\"}} {}",
        snap.admissions.refused_expired_total
    );

    let _ = writeln!(out, "# HELP gtt_relay_admissions_sync_dropped_total Admissions dropped during sync due to capacity.");
    let _ = writeln!(
        out,
        "# TYPE gtt_relay_admissions_sync_dropped_total counter"
    );
    let _ = writeln!(
        out,
        "gtt_relay_admissions_sync_dropped_total{{{base_labels}}} {}",
        snap.admissions.sync_dropped_capacity_total
    );

    let _ = writeln!(out, "# HELP gtt_relay_admissions_revocations_enforced_total Revocations enforced from control plane sync.");
    let _ = writeln!(
        out,
        "# TYPE gtt_relay_admissions_revocations_enforced_total counter"
    );
    let _ = writeln!(
        out,
        "gtt_relay_admissions_revocations_enforced_total{{{base_labels}}} {}",
        snap.admissions.revocations_enforced_total
    );

    out
}

/// Escape Prometheus label values (`\` -> `\\`, `"` -> `\"`, `\n` -> `\n`).
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
