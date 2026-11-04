//! Unit tests for relay observability and Prometheus exposition (`T-103`, `OPS-002`, `OPS-006`).

use gtt_relay::{
    format_relay_prometheus, RelayAdmissionMetrics, RelayLimitMetrics, RelayTcpMetrics,
    RelayTelemetrySnapshot, RelayUdpMetrics,
};

#[test]
fn test_relay_prometheus_formatting_renders_required_metrics_and_labels() {
    let snapshot = RelayTelemetrySnapshot {
        relay_id: "relay-staging-01".to_string(),
        region: "staging-us-east".to_string(),
        uptime_secs: 12345,
        sessions_active: 8,
        admissions: RelayAdmissionMetrics {
            admitted_total: 100,
            refused_unadmitted_total: 5,
            refused_expired_total: 2,
            sync_dropped_capacity_total: 0,
            revocations_enforced_total: 3,
        },
        limits: RelayLimitMetrics {
            queued_frames: 42,
            queued_bytes: 40960,
            peak_session_queued_bytes: 81920,
            frames_enqueued: 50000,
            bytes_enqueued: 25000000,
            frames_dropped: 12,
            dropped_queue_frames_full: 4,
            dropped_queue_bytes_full: 3,
            dropped_global_memory_cap: 2,
            dropped_bandwidth_exceeded: 3,
        },
        tcp: RelayTcpMetrics {
            active_tcp_flows: 15,
            total_connected: 250,
            total_refused: 4,
            total_limit_reached: 1,
            bytes_guest_to_engine: 50_000_000,
            bytes_engine_to_guest: 120_000_000,
        },
        udp: RelayUdpMetrics {
            active_udp_flows: 30,
            datagrams_guest_to_engine: 80000,
            datagrams_engine_to_guest: 95000,
            datagrams_dropped: 15,
            dropped_oversize: 2,
            dropped_unknown_mapping: 3,
            dropped_malformed: 1,
            dropped_flow_limit: 4,
            dropped_unknown_flow: 5,
            flows_expired: 50,
        },
        global_queued_bytes: 1_048_576,         // 1 MiB
        global_memory_budget_bytes: 16_777_216, // 16 MiB
    };

    let body = format_relay_prometheus(&snapshot);

    // Verify key lines exist with proper labels and values
    assert!(body.contains(
        "gtt_relay_uptime_seconds{relay_id=\"relay-staging-01\",region=\"staging-us-east\"} 12345"
    ));
    assert!(body.contains(
        "gtt_relay_sessions_active{relay_id=\"relay-staging-01\",region=\"staging-us-east\"} 8"
    ));
    assert!(body.contains(
        "gtt_relay_tcp_flows_active{relay_id=\"relay-staging-01\",region=\"staging-us-east\"} 15"
    ));
    assert!(body.contains(
        "gtt_relay_udp_flows_active{relay_id=\"relay-staging-01\",region=\"staging-us-east\"} 30"
    ));

    // Egress bytes (cost driver)
    assert!(body.contains("gtt_relay_egress_bytes_total{relay_id=\"relay-staging-01\",region=\"staging-us-east\",protocol=\"tcp\"} 120000000"));

    // Saturation drop counters
    assert!(body.contains("gtt_relay_frames_dropped_total{relay_id=\"relay-staging-01\",region=\"staging-us-east\",reason=\"bandwidth_exceeded\"} 3"));
    assert!(body.contains("gtt_relay_frames_dropped_total{relay_id=\"relay-staging-01\",region=\"staging-us-east\",reason=\"global_memory_cap\"} 2"));

    // Global memory utilization
    assert!(body.contains("gtt_relay_global_memory_utilization_ratio{relay_id=\"relay-staging-01\",region=\"staging-us-east\"} 0.0625"));

    // Admission metrics
    assert!(body.contains("gtt_relay_admissions_checks_total{relay_id=\"relay-staging-01\",region=\"staging-us-east\",result=\"admitted\"} 100"));
    assert!(body.contains("gtt_relay_admissions_checks_total{relay_id=\"relay-staging-01\",region=\"staging-us-east\",result=\"refused_unadmitted\"} 5"));
}
