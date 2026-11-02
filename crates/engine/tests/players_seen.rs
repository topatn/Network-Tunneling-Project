//! Integration tests for `T-073` (`players_seen` from relay `flow_summary`, `FR-012`, `FR-010`,
//! `UX-002`).

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::time::Duration;

use gtt_engine::{
    EngineEvent, EngineLifecycle, MappingTargetInput, ProbeStatus, OBSERVED_TRAFFIC_LABEL,
};
use gtt_profiles::Registry;
use gtt_protocol::{
    encode_control_frame, ControlMessage, EngineSessionState, HelloAckMapping, HelloAckMessage,
    Protocol, SessionLimits, StartTunnelResponse,
};
use gtt_relay::RelayTunnelSession;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn repo_profiles_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("profiles")
}

#[test]
fn scripted_guests_drive_flow_summary_and_engine_players_seen_matches_exact_guest_count(
) -> TestResult {
    let registry = Registry::load_dir(&repo_profiles_dir())?;
    let mc_java = registry
        .get("minecraft-java")
        .ok_or("missing minecraft-java profile")?;

    // Bind a local TCP listener so we can also verify three-layer indicator independence (`FR-010`,
    // `UX-002`).
    let local_tcp = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
    let local_port = local_tcp.local_addr()?.port();

    let inputs = vec![
        MappingTargetInput {
            mapping_key: "game".to_owned(),
            local_host: "127.0.0.1".to_owned(),
            local_port,
            enabled: true,
            confirm_non_loopback: false,
        },
        MappingTargetInput {
            mapping_key: "query".to_owned(),
            local_host: "127.0.0.1".to_owned(),
            local_port: 25565,
            enabled: true,
            confirm_non_loopback: false,
        },
    ];

    let mut engine = EngineLifecycle::configure(mc_java, &inputs, "0.1.0-test")
        .map_err(|e| format!("configure failed: {e:?}"))?;

    let start_resp: StartTunnelResponse = serde_json::from_value(serde_json::json!({
        "tunnel_id": "11111111-1111-4111-8111-111111111111",
        "allocation_token": "eyJhbGciOiJFZERTQSJ9.eyJpc3MiOiJndHQifQ.c2ln",
        "allocation": {
            "id": "22222222-2222-4222-8222-222222222222",
            "expires_at": "2026-10-20T12:00:00Z",
            "relay": {
                "region": "us-east",
                "host": "relay.us-east.example.test",
                "tunnel_port": 4433
            },
            "ports": [
                { "mapping_key": "game", "protocol": "tcp", "public_port": 25565 },
                { "mapping_key": "query", "protocol": "udp", "public_port": 25565 }
            ]
        }
    }))?;

    let _ = engine.begin_start_with_response(&start_resp)?;
    let _ = engine.on_quic_connected()?;

    let mappings = vec![
        HelloAckMapping {
            index: 0,
            mapping_key: "game".to_owned(),
            protocol: Protocol::Tcp,
            public_port: 25565,
        },
        HelloAckMapping {
            index: 1,
            mapping_key: "query".to_owned(),
            protocol: Protocol::Udp,
            public_port: 25565,
        },
    ];
    let limits = SessionLimits {
        max_tcp_flows: 32,
        max_udp_flows: 32,
    };

    let hello_ack = encode_control_frame(&ControlMessage::HelloAck(HelloAckMessage {
        session_id: "3f2b8c1e-5a47-4d0b-9c1e-0a6f1d2e7b44".to_owned(),
        heartbeat_interval_ms: 5_000,
        idle_timeout_ms: 15_000,
        limits,
        mappings: mappings.clone(),
    }))?;
    let _ = engine.handle_relay_control_frame(&hello_ack, 100)?;
    assert_eq!(engine.state(), EngineSessionState::Active);

    // Initialize RelayTunnelSession (`T-063`) to generate authoritative `flow_summary` frames as
    // scripted guests arrive.
    let mut relay_session = RelayTunnelSession::new(
        "22222222-2222-4222-8222-222222222222",
        "11111111-1111-4111-8111-111111111111",
        "3f2b8c1e-5a47-4d0b-9c1e-0a6f1d2e7b44",
        mappings,
        5_000,
        15_000,
        30_000,
        limits,
        100,
    );

    // Wave 0: 0 scripted guests initially.
    assert_eq!(engine.players_seen().distinct_sources, 0);
    assert_eq!(engine.players_seen().label, OBSERVED_TRAFFIC_LABEL);

    // Wave 1: 3 scripted TCP guests (`203.0.113.10`, `203.0.113.11`, `203.0.113.12`) connect to
    // mapping 0 (`game`). Guest 1 (`203.0.113.10`) opens two TCP streams to verify distinct source
    // deduplication.
    let tcp_guests = [
        IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10)),
        IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10)),
        IpAddr::V4(Ipv4Addr::new(203, 0, 113, 11)),
        IpAddr::V4(Ipv4Addr::new(203, 0, 113, 12)),
    ];
    for &guest_ip in &tcp_guests {
        let _ = relay_session.tcp.begin_tcp_open(0, guest_ip)?;
    }

    let summary_wave_1 = relay_session.build_flow_summary();
    assert_eq!(summary_wave_1.tcp_flows, 4);
    assert_eq!(summary_wave_1.udp_flows, 0);
    assert_eq!(summary_wave_1.distinct_sources, 3);

    let frame_wave_1 = encode_control_frame(&ControlMessage::FlowSummary(summary_wave_1))?;
    let _ = engine.handle_relay_control_frame(&frame_wave_1, 500)?;

    assert_eq!(engine.players_seen().distinct_sources, 3);
    assert_eq!(engine.players_seen().tcp_flows, 4);
    assert_eq!(engine.players_seen().udp_flows, 0);
    assert_eq!(engine.players_seen().label, OBSERVED_TRAFFIC_LABEL);

    // Wave 2: 2 additional scripted UDP guests (`198.51.100.21`, `198.51.100.22`) send datagrams
    // to mapping 1 (`query`), bringing total scripted guests to 5.
    let udp_guests = [
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 21)), 50001),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 22)), 50002),
    ];
    for &guest_addr in &udp_guests {
        let _ = relay_session
            .udp
            .handle_guest_datagram(1, guest_addr, b"query-pkt", 600)
            .ok_or("expected UDP datagram")?;
    }

    let summary_wave_2 = relay_session.build_flow_summary();
    assert_eq!(summary_wave_2.tcp_flows, 4);
    assert_eq!(summary_wave_2.udp_flows, 2);
    assert_eq!(summary_wave_2.distinct_sources, 5);

    let frame_wave_2 = encode_control_frame(&ControlMessage::FlowSummary(summary_wave_2))?;
    let _ = engine.handle_relay_control_frame(&frame_wave_2, 700)?;

    // Verify count equals the number of scripted guests (5) and carries the observed-traffic label
    // (`T-073`, `FR-012`).
    assert_eq!(engine.players_seen().distinct_sources, 5);
    assert_eq!(engine.players_seen().tcp_flows, 4);
    assert_eq!(engine.players_seen().udp_flows, 2);
    assert_eq!(engine.players_seen().label, OBSERVED_TRAFFIC_LABEL);

    // Verify `EngineEvent::PlayersSeen` was emitted with the exact count and label.
    let players_events: Vec<_> = engine
        .drain_events()
        .into_iter()
        .filter_map(|e| match e {
            EngineEvent::PlayersSeen(snap) => Some(snap),
            _ => None,
        })
        .collect();
    assert_eq!(players_events.len(), 2);
    assert_eq!(players_events.last().map(|s| s.distinct_sources), Some(5));
    assert_eq!(
        players_events.last().map(|s| s.label),
        Some(OBSERVED_TRAFFIC_LABEL)
    );

    // Verify three-layer indicator independence (`FR-010`, `UX-002`): probing local reachability
    // updates `local_probes` without changing `tunnel_state` or `players_seen`.
    let _ = engine.probe_local(Some("game"), Duration::from_millis(250));
    let snapshot = engine.get_status();
    assert_eq!(snapshot.tunnel_state, EngineSessionState::Active);
    assert_eq!(snapshot.players_seen.distinct_sources, 5);
    assert_eq!(
        snapshot.local_probes.first().map(|p| p.status),
        Some(ProbeStatus::Reachable)
    );

    // Drop the local TCP server and re-probe: `local_probes` becomes `Unreachable` while
    // `tunnel_state` stays `Active` and `players_seen.distinct_sources` stays `5`.
    drop(local_tcp);
    let _ = engine.probe_local(Some("game"), Duration::from_millis(200));
    let snapshot_after_local_stop = engine.get_status();
    assert_eq!(
        snapshot_after_local_stop.tunnel_state,
        EngineSessionState::Active
    );
    assert_eq!(snapshot_after_local_stop.players_seen.distinct_sources, 5);
    assert_eq!(
        snapshot_after_local_stop
            .local_probes
            .first()
            .map(|p| p.status),
        Some(ProbeStatus::Unreachable)
    );

    Ok(())
}
