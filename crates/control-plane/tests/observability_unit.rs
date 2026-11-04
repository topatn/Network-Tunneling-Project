//! Unit tests for control plane observability, Prometheus exposition, and redacted structured logging
//! (`T-103`, `OPS-002`, `OPS-006`, `PRIV-002`, `SEC-013`).

use gtt_control_plane::{
    format_control_plane_prometheus, format_structured_log, ControlPlaneTelemetrySnapshot,
    RelayNodeTelemetry,
};
use serde_json::json;

#[test]
fn test_control_plane_prometheus_formatting_renders_expected_metrics_and_labels() {
    let snapshot = ControlPlaneTelemetrySnapshot {
        environment: "staging".to_string(),
        version: "0.1.0-alpha".to_string(),
        uptime_secs: 7200,
        active_tunnels: 24,
        allocation_requests_total: 150,
        allocation_failures_total: 5,
        allocation_failures_by_reason: vec![
            ("no_healthy_relay".to_string(), 2),
            ("port_exhaustion".to_string(), 2),
            ("quota_exceeded".to_string(), 1),
        ],
        rate_limit_rejections: vec![("strict".to_string(), 8), ("normal".to_string(), 3)],
        relays: vec![
            RelayNodeTelemetry {
                relay_id: "relay-staging-01".to_string(),
                region: "staging-us-east".to_string(),
                status: "healthy".to_string(),
                last_heartbeat_epoch_s: 1730000000,
                ports_allocated: 920,
                ports_capacity: 1000,
                egress_bytes_recorded: 54_000_000_000, // 54 GB
            },
            RelayNodeTelemetry {
                relay_id: "relay-staging-02".to_string(),
                region: "staging-eu-central".to_string(),
                status: "unhealthy".to_string(),
                last_heartbeat_epoch_s: 1729999000,
                ports_allocated: 120,
                ports_capacity: 1000,
                egress_bytes_recorded: 12_000_000_000,
            },
        ],
    };

    let body = format_control_plane_prometheus(&snapshot);

    // 1. Process info & active tunnels
    assert!(
        body.contains("gtt_control_plane_info{environment=\"staging\",version=\"0.1.0-alpha\"} 1")
    );
    assert!(body.contains("gtt_control_plane_tunnels_active{environment=\"staging\"} 24"));

    // 2. Allocation metrics
    assert!(
        body.contains("gtt_control_plane_allocation_requests_total{environment=\"staging\"} 150")
    );
    assert!(body.contains("gtt_control_plane_allocation_failures_total{environment=\"staging\",reason=\"no_healthy_relay\"} 2"));
    assert!(body.contains("gtt_control_plane_allocation_failures_total{environment=\"staging\",reason=\"port_exhaustion\"} 2"));
    assert!(body.contains("gtt_control_plane_allocation_failures_total{environment=\"staging\",reason=\"quota_exceeded\"} 1"));

    // 3. Rate limiting rejections
    assert!(body.contains(
        "gtt_control_plane_rate_limit_rejections_total{environment=\"staging\",class=\"strict\"} 8"
    ));
    assert!(body.contains(
        "gtt_control_plane_rate_limit_rejections_total{environment=\"staging\",class=\"normal\"} 3"
    ));

    // 4. Per-relay status & heartbeats
    assert!(body.contains("gtt_control_plane_relay_status{environment=\"staging\",relay_id=\"relay-staging-01\",region=\"staging-us-east\",status=\"healthy\"} 1"));
    assert!(body.contains("gtt_control_plane_relay_status{environment=\"staging\",relay_id=\"relay-staging-01\",region=\"staging-us-east\",status=\"unhealthy\"} 0"));
    assert!(body.contains("gtt_control_plane_relay_status{environment=\"staging\",relay_id=\"relay-staging-02\",region=\"staging-eu-central\",status=\"unhealthy\"} 1"));
    assert!(body.contains("gtt_control_plane_relay_heartbeat_timestamp_seconds{environment=\"staging\",relay_id=\"relay-staging-01\",region=\"staging-us-east\"} 1730000000"));

    // 5. Port pool saturation
    assert!(body.contains("gtt_control_plane_relay_ports_allocated{environment=\"staging\",relay_id=\"relay-staging-01\",region=\"staging-us-east\"} 920"));
    assert!(body.contains("gtt_control_plane_relay_ports_capacity{environment=\"staging\",relay_id=\"relay-staging-01\",region=\"staging-us-east\"} 1000"));
    assert!(body.contains("gtt_control_plane_relay_port_pool_utilization_ratio{environment=\"staging\",relay_id=\"relay-staging-01\",region=\"staging-us-east\"} 0.9200"));

    // 6. Egress bytes recorded (OPS-006)
    assert!(body.contains("gtt_control_plane_relay_egress_bytes_recorded_total{environment=\"staging\",relay_id=\"relay-staging-01\",region=\"staging-us-east\"} 54000000000"));
}

#[test]
fn test_structured_log_redaction_guarantees_zero_token_ip_or_secret_leaks() {
    let fields = json!({
        "tunnel_id": "33333333-3333-4333-8333-333333333333",
        "access_token": "gtt_act_secret_token_12345",
        "secret": "gtt_inv_secret_plaintext_value",
        "guest_ip": "198.51.100.42",
        "safe_region": "staging-us-east",
        "code_verifier": "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
    });

    let log_line = format_structured_log(
        "2026-11-04T12:00:00Z",
        "info",
        "routes::invitations",
        "invitation_claimed",
        fields,
    );

    // Must be valid JSON
    let parsed_res: Result<serde_json::Value, _> = serde_json::from_str(&log_line);
    assert!(parsed_res.is_ok());
    let Ok(parsed) = parsed_res else {
        return;
    };

    assert_eq!(
        parsed.get("event").and_then(|v| v.as_str()),
        Some("invitation_claimed")
    );

    let fields_obj = parsed.get("fields").and_then(|v| v.as_object());
    assert!(fields_obj.is_some());

    if let Some(fields) = fields_obj {
        // Sensitive keys must be redacted
        assert_eq!(
            fields.get("access_token").and_then(|v| v.as_str()),
            Some("[REDACTED-SECRET]")
        );
        assert_eq!(
            fields.get("secret").and_then(|v| v.as_str()),
            Some("[REDACTED-SECRET]")
        );
        assert_eq!(
            fields.get("code_verifier").and_then(|v| v.as_str()),
            Some("[REDACTED-SECRET]")
        );
        assert_eq!(
            fields.get("guest_ip").and_then(|v| v.as_str()),
            Some("[REDACTED-IP]")
        );
        assert_eq!(
            fields.get("safe_region").and_then(|v| v.as_str()),
            Some("staging-us-east")
        );
    }

    // Plaintext secret/IP strings must NOT appear anywhere in the output
    assert!(!log_line.contains("gtt_act_secret_token_12345"));
    assert!(!log_line.contains("gtt_inv_secret_plaintext_value"));
    assert!(!log_line.contains("198.51.100.42"));
    assert!(!log_line.contains("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"));
}
