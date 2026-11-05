//! Integration tests for Engine <-> UI bridge coordinator
//! (`T-070`, `T-073`, `T-080`, `T-083`, `contracts.md` §13, `FR-005`, `FR-010`, `FR-011`, `FR-012`).

use gtt_desktop::engine_bridge::{
    ConfigureMappingsRequest, EngineBridge, MappingTargetInputDto, ProbeLocalRequestDto,
    StartTunnelRequestDto, StopTunnelRequestDto,
};
use gtt_engine::{CONFIRMED_PRESENCE_LABEL, OBSERVED_TRAFFIC_LABEL};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn t070_engine_bridge_initializes_in_idle_and_exposes_status() -> TestResult {
    let bridge = EngineBridge::new().map_err(|e| format!("bridge new failed: {e}"))?;
    let status = bridge
        .get_status()
        .map_err(|e| format!("get_status failed: {e}"))?;

    // Initial state invariants
    assert_eq!(status.tunnel_state, "Not running");
    assert_eq!(status.raw_state, "Idle");
    assert!(status.tunnel_id.is_none());
    assert!(status.allocation_id.is_none());
    assert!(status.public_endpoints.is_empty());
    assert_eq!(status.players_seen.distinct_sources, 0);
    assert_eq!(status.players_seen.label, OBSERVED_TRAFFIC_LABEL);
    assert_eq!(status.confirmed_presence.label, CONFIRMED_PRESENCE_LABEL);
    assert_eq!(status.connectivity_mode, "GttRelay");

    Ok(())
}

#[test]
fn t070_engine_bridge_configures_mappings_and_enforces_target_policy() -> TestResult {
    let bridge = EngineBridge::new().map_err(|e| format!("bridge new failed: {e}"))?;

    // 1. Valid configuration on Minecraft: Java Edition
    let valid_req = ConfigureMappingsRequest {
        profile_id: "minecraft-java".to_owned(),
        mappings: vec![MappingTargetInputDto {
            mapping_key: "game".to_owned(),
            local_host: "127.0.0.1".to_owned(),
            local_port: 25565,
            enabled: true,
            confirm_non_loopback: false,
        }],
    };
    let resp = bridge
        .configure_mappings(valid_req)
        .map_err(|e| format!("configure failed: {e}"))?;
    assert!(resp.ok);
    assert!(resp.validation_errors.is_empty());
    assert_eq!(resp.profile_id, "minecraft-java");

    // 2. Invalid configuration attempting to dial sensitive port 22 (SSH) without profile declaration
    let bad_port_req = ConfigureMappingsRequest {
        profile_id: "minecraft-java".to_owned(),
        mappings: vec![MappingTargetInputDto {
            mapping_key: "game".to_owned(),
            local_host: "127.0.0.1".to_owned(),
            local_port: 22, // Sensitive port deny-list (SEC-006)
            enabled: true,
            confirm_non_loopback: false,
        }],
    };
    let resp = bridge
        .configure_mappings(bad_port_req)
        .map_err(|e| format!("configure failed: {e}"))?;
    assert!(!resp.ok);
    assert!(!resp.validation_errors.is_empty());

    // 3. Invalid non-loopback IP without explicit confirmation
    let non_loopback_unconfirmed = ConfigureMappingsRequest {
        profile_id: "minecraft-java".to_owned(),
        mappings: vec![MappingTargetInputDto {
            mapping_key: "game".to_owned(),
            local_host: "192.168.1.50".to_owned(),
            local_port: 25565,
            enabled: true,
            confirm_non_loopback: false, // Must be explicitly confirmed (FR-003, SEC-006)
        }],
    };
    let resp = bridge
        .configure_mappings(non_loopback_unconfirmed)
        .map_err(|e| format!("configure failed: {e}"))?;
    assert!(!resp.ok);
    assert!(!resp.validation_errors.is_empty());

    Ok(())
}

#[test]
fn t070_t073_engine_bridge_drives_full_tunnel_session_lifecycle() -> TestResult {
    let bridge = EngineBridge::new().map_err(|e| format!("bridge new failed: {e}"))?;

    // 1. Configure tunnel mapping
    let config_req = ConfigureMappingsRequest {
        profile_id: "minecraft-java".to_owned(),
        mappings: vec![MappingTargetInputDto {
            mapping_key: "game".to_owned(),
            local_host: "127.0.0.1".to_owned(),
            local_port: 25565,
            enabled: true,
            confirm_non_loopback: false,
        }],
    };
    bridge
        .configure_mappings(config_req)
        .map_err(|e| format!("configure failed: {e}"))?;

    // 2. Start tunnel: Idle -> Connecting -> Authenticating -> Active (Connected)
    let start_req = StartTunnelRequestDto {
        mode: Some("relay".to_owned()),
        tunnel_id: Some("11111111-1111-4111-8111-111111111111".to_owned()),
        allocation_token: Some("mock_token_abc".to_owned()),
        relay_host: Some("relay.us-east.example.test".to_owned()),
        public_port: Some(25565),
    };
    let start_status = bridge
        .start_tunnel(start_req)
        .map_err(|e| format!("start failed: {e}"))?;
    assert_eq!(start_status.tunnel_state, "Connected");
    assert_eq!(start_status.raw_state, "Active");
    assert_eq!(start_status.public_endpoints.len(), 1);

    let ep = start_status
        .public_endpoints
        .first()
        .ok_or("expected 1 endpoint")?;
    assert_eq!(ep.mapping_key, "game");
    assert_eq!(ep.protocol, "tcp");
    assert_eq!(ep.relay_host, "relay.us-east.example.test");
    assert_eq!(ep.public_port, 25565);
    assert_eq!(ep.display_address, "relay.us-east.example.test:25565");

    // 3. Verify events were emitted
    let events = bridge
        .drain_events()
        .map_err(|e| format!("drain_events failed: {e}"))?;
    assert!(!events.is_empty());
    assert!(events
        .iter()
        .any(|ev| ev.event_type == "tunnel_status_changed"));

    // 4. Stop tunnel: Active -> Stopped (Not running)
    let stop_req = StopTunnelRequestDto {
        reason: Some("user requested stop".to_owned()),
    };
    let stop_status = bridge
        .stop_tunnel(stop_req)
        .map_err(|e| format!("stop failed: {e}"))?;
    assert_eq!(stop_status.tunnel_state, "Not running");
    assert_eq!(stop_status.raw_state, "Stopped");
    assert_eq!(stop_status.players_seen.distinct_sources, 0);

    Ok(())
}

#[test]
fn t072_local_probe_and_diagnostics_bundle_export() -> TestResult {
    let bridge = EngineBridge::new().map_err(|e| format!("bridge new failed: {e}"))?;

    // 1. Run probe on unlistened local target (expect ok == false without panicking)
    let probe_req = ProbeLocalRequestDto {
        mapping_key: Some("game".to_owned()),
    };
    let probes = bridge
        .probe_local(probe_req)
        .map_err(|e| format!("probe_local failed: {e}"))?;
    assert_eq!(probes.len(), 1);
    let p = probes.first().ok_or("expected 1 probe")?;
    assert_eq!(p.mapping_key, "game");
    assert_eq!(p.level, "L0");
    assert_eq!(p.status, "unreachable");
    assert!(!p.ok);

    // 2. Export diagnostics bundle and verify privacy redaction
    let diag_json = bridge
        .export_diagnostics()
        .map_err(|e| format!("export_diagnostics failed: {e}"))?;
    assert!(!diag_json.is_empty());
    assert!(diag_json.contains("schema_version"));
    assert!(!diag_json.contains("mock_token_abc"));

    // 3. List provisionable profiles
    let provisionable = bridge
        .list_provisionable()
        .map_err(|e| format!("list_provisionable failed: {e}"))?;
    assert!(provisionable
        .iter()
        .any(|p| p.profile_id == "minecraft-java"));

    Ok(())
}

#[test]
fn t080_handle_http_request_dispatches_all_engine_routes() -> TestResult {
    let bridge = EngineBridge::new().map_err(|e| format!("bridge new failed: {e}"))?;

    // 1. OPTIONS CORS preflight
    let (status, _, _) = bridge.handle_http_request("OPTIONS", "/api/engine/status", &[]);
    assert_eq!(status, 204);

    // 2. GET /api/engine/status
    let (status, ctype, body) = bridge.handle_http_request("GET", "/api/engine/status", &[]);
    assert_eq!(status, 200);
    assert_eq!(ctype, "application/json; charset=utf-8");
    let status_json: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(
        status_json.get("tunnel_state").and_then(|v| v.as_str()),
        Some("Not running")
    );

    // 3. POST /api/engine/configure
    let cfg_body = serde_json::json!({
        "profile_id": "minecraft-java",
        "mappings": [{
            "mapping_key": "game",
            "local_host": "127.0.0.1",
            "local_port": 25565,
            "enabled": true,
            "confirm_non_loopback": false
        }]
    });
    let (status, _, body) = bridge.handle_http_request(
        "POST",
        "/api/engine/configure",
        &serde_json::to_vec(&cfg_body)?,
    );
    assert_eq!(status, 200);
    let cfg_resp: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(cfg_resp.get("ok").and_then(|v| v.as_bool()), Some(true));

    // 4. POST /api/engine/start
    let start_body = serde_json::json!({
        "mode": "relay",
        "relay_host": "relay.us-east.example.test",
        "public_port": 25565
    });
    let (status, _, body) = bridge.handle_http_request(
        "POST",
        "/api/engine/start",
        &serde_json::to_vec(&start_body)?,
    );
    assert_eq!(status, 200);
    let start_resp: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(
        start_resp.get("tunnel_state").and_then(|v| v.as_str()),
        Some("Connected")
    );
    assert_eq!(
        start_resp.get("raw_state").and_then(|v| v.as_str()),
        Some("Active")
    );

    // 5. POST /api/engine/probe
    let (status, _, _) = bridge.handle_http_request("POST", "/api/engine/probe", b"{}");
    assert_eq!(status, 200);

    // 6. GET /api/engine/events
    let (status, _, body) = bridge.handle_http_request("GET", "/api/engine/events", &[]);
    assert_eq!(status, 200);
    let events: serde_json::Value = serde_json::from_slice(&body)?;
    assert!(events.is_array());

    // 7. POST /api/engine/stop
    let (status, _, body) = bridge.handle_http_request("POST", "/api/engine/stop", b"{}");
    assert_eq!(status, 200);
    let stop_resp: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(
        stop_resp.get("tunnel_state").and_then(|v| v.as_str()),
        Some("Not running")
    );

    // 8. GET /api/engine/provisionable
    let (status, _, _) = bridge.handle_http_request("GET", "/api/engine/provisionable", &[]);
    assert_eq!(status, 200);

    // 9. GET /api/engine/diagnostics
    let (status, _, _) = bridge.handle_http_request("GET", "/api/engine/diagnostics", &[]);
    assert_eq!(status, 200);

    // 10. Unknown route returns 404
    let (status, _, _) = bridge.handle_http_request("GET", "/api/engine/unknown", &[]);
    assert_eq!(status, 404);

    Ok(())
}

#[test]
fn t076_engine_bridge_managed_server_routes_lifecycle() -> TestResult {
    let bridge = EngineBridge::new().map_err(|e| format!("bridge new failed: {e}"))?;

    // 1. Initial server status
    let (status, _, body) = bridge.handle_http_request("GET", "/api/engine/server/status", &[]);
    assert_eq!(status, 200);
    let s_resp: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(s_resp.get("ok").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
        s_resp.get("state").and_then(|v| v.as_str()),
        Some("not_installed")
    );

    // 2. Provision with official server.jar if present on host
    let jar_path = r"C:\Users\chris\Downloads\server.jar";
    if std::path::Path::new(jar_path).exists() {
        let prov_payload = serde_json::json!({
            "profile_id": "minecraft-java",
            "jar_path": jar_path,
            "accept_eula": true,
            "version": "26.3",
            "max_players": 20
        });
        let (status, _, body) = bridge.handle_http_request(
            "POST",
            "/api/engine/server/provision",
            &serde_json::to_vec(&prov_payload)?,
        );
        assert_eq!(status, 200);
        let prov_resp: serde_json::Value = serde_json::from_slice(&body)?;
        assert_eq!(prov_resp.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            prov_resp.get("server_port").and_then(|v| v.as_u64()),
            Some(25565)
        );

        // 3. Stop
        let (status, _, body) =
            bridge.handle_http_request("POST", "/api/engine/server/stop", b"{}");
        assert_eq!(status, 200);
        let stop_resp: serde_json::Value = serde_json::from_slice(&body)?;
        assert_eq!(stop_resp.get("ok").and_then(|v| v.as_bool()), Some(true));
    }

    Ok(())
}
