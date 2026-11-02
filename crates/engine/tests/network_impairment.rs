//! Network impairment and IPv6 test suite (`T-092`, `TEST-003`, `NET-005`, `NET-010`, `FR-014`).
//!
//! Covers:
//! 1. **Latency & Jitter Injection (`NET-005`, `TEST-003`):** Heartbeat RTT measurement across
//!    variable network delay; transitions `Active <-> Degraded` across threshold (250 ms).
//! 2. **Packet Loss & Silent-Peer Idle Timeout (`NET-005`, `FR-014`):** Missing heartbeats trigger
//!    `Degraded` -> `Reconnecting` (Engine) / `DisconnectedInGrace` (Relay); immediate TCP closure
//!    and UDP flow retention.
//! 3. **Blocked UDP Transport (`TEST-003`, `NET-007`):** Dropped UDP datagrams do not affect TCP
//!    streams or control channel; bounded memory under loss.
//! 4. **Restricted NAT / Outage Resumption within Grace vs Expiry (`NET-004`, `NET-008`):**
//!    Resumption within grace keeps public ports and UDP continuity; reconnect after grace
//!    is refused and resources swept.
//! 5. **IPv4 and IPv6 Dual-Stack and Address Mismatch (`NET-010`, `FR-008`, `SEC-009`, `E-13`):**
//!    Native IPv4, native IPv6, IPv4-mapped IPv6 normalization, and the documented outcome of
//!    claiming over IPv6 and connecting over IPv4 (and vice-versa).

use std::error::Error;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use gtt_engine::{EngineTunnelSession, LocalMappingConfig};
use gtt_protocol::{
    encode_udp_datagram, EngineSessionState, HelloAckMapping, HelloAckMessage, Protocol,
    SequenceMessage, SessionLimits, TcpOpenHeader, DEFAULT_MAX_DATAGRAM_BYTES,
};
use gtt_relay::{
    canonicalize_guest_ip, RelayAdmissionEnforcer, RelayAdmissionSyncDelta,
    RelaySessionConnectivity, RelayTunnelSession,
};

type TestResult = Result<(), Box<dyn Error>>;

const TUNNEL_ID_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const TUNNEL_ID_B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const ALLOCATION_ID: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
const SESSION_ID_1: &str = "11111111-1111-4111-8111-111111111111";
const SESSION_ID_2: &str = "22222222-2222-4222-8222-222222222222";

fn test_mappings() -> Vec<HelloAckMapping> {
    vec![
        HelloAckMapping {
            index: 0,
            mapping_key: "minecraft-java".to_owned(),
            protocol: Protocol::Tcp,
            public_port: 25565,
        },
        HelloAckMapping {
            index: 1,
            mapping_key: "minecraft-bedrock".to_owned(),
            protocol: Protocol::Udp,
            public_port: 19132,
        },
    ]
}

fn sample_ack(session_id: &str) -> HelloAckMessage {
    HelloAckMessage {
        session_id: session_id.to_owned(),
        heartbeat_interval_ms: 5_000,
        idle_timeout_ms: 15_000,
        limits: SessionLimits {
            max_tcp_flows: 16,
            max_udp_flows: 16,
        },
        mappings: test_mappings(),
    }
}

// ---------------------------------------------------------------------------
// 1. Latency & Jitter Injection Test (NET-005, TEST-003)
// ---------------------------------------------------------------------------

#[test]
fn test_latency_and_jitter_injection_transitions_to_degraded_and_recovers() -> TestResult {
    let mut session = EngineTunnelSession::new(
        vec![
            LocalMappingConfig::new(
                0,
                "minecraft-java",
                Protocol::Tcp,
                true,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 25565),
            ),
            LocalMappingConfig::new(
                1,
                "minecraft-bedrock",
                Protocol::Udp,
                true,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 19132),
            ),
        ],
        16,
        16,
    )?;

    // Transition Idle -> Connecting -> Authenticating -> Active.
    assert_eq!(session.state(), EngineSessionState::Idle);
    assert_eq!(session.start()?, EngineSessionState::Connecting);
    assert_eq!(
        session.on_quic_established()?,
        EngineSessionState::Authenticating
    );
    assert_eq!(
        session.on_hello_ack(sample_ack(SESSION_ID_1), 1_000)?,
        EngineSessionState::Active
    );

    // Baseline RTT (35 ms <= 250 ms threshold): remains Active.
    let _ = session.build_ping(2_000);
    assert_eq!(session.on_pong(SequenceMessage { seq: 1 }, 2_035), Some(35));
    assert_eq!(session.state(), EngineSessionState::Active);
    assert_eq!(session.last_rtt_ms(), Some(35));

    // High latency spike / jitter (340 ms > 250 ms threshold): transitions to Degraded.
    let _ = session.build_ping(3_000);
    assert_eq!(
        session.on_pong(SequenceMessage { seq: 2 }, 3_340),
        Some(340)
    );
    assert_eq!(session.state(), EngineSessionState::Degraded);
    assert_eq!(session.last_rtt_ms(), Some(340));

    // Continued jitter (275 ms > 250 ms): stays in Degraded.
    let _ = session.build_ping(4_000);
    assert_eq!(
        session.on_pong(SequenceMessage { seq: 3 }, 4_275),
        Some(275)
    );
    assert_eq!(session.state(), EngineSessionState::Degraded);

    // Channel latency recovers (42 ms <= 250 ms): transitions back to Active.
    let _ = session.build_ping(5_000);
    assert_eq!(session.on_pong(SequenceMessage { seq: 4 }, 5_042), Some(42));
    assert_eq!(session.state(), EngineSessionState::Active);
    assert_eq!(session.last_rtt_ms(), Some(42));

    Ok(())
}

// ---------------------------------------------------------------------------
// 2. Packet Loss & Liveness Timeout Test (NET-005, FR-014, TEST-003)
// ---------------------------------------------------------------------------

#[test]
fn test_packet_loss_heartbeat_idle_timeout_and_transport_split_behavior() -> TestResult {
    let local_server = UdpSocket::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    local_server.set_read_timeout(Some(Duration::from_millis(500)))?;
    let local_server_addr = local_server.local_addr()?;

    let mut engine_session = EngineTunnelSession::new(
        vec![
            LocalMappingConfig::new(
                0,
                "minecraft-java",
                Protocol::Tcp,
                true,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 25565),
            ),
            LocalMappingConfig::new(
                1,
                "minecraft-bedrock",
                Protocol::Udp,
                true,
                local_server_addr,
            ),
        ],
        16,
        16,
    )?;

    assert_eq!(engine_session.start()?, EngineSessionState::Connecting);
    assert_eq!(
        engine_session.on_quic_established()?,
        EngineSessionState::Authenticating
    );
    assert_eq!(
        engine_session.on_hello_ack(sample_ack(SESSION_ID_1), 1_000)?,
        EngineSessionState::Active
    );

    let mut relay_session = RelayTunnelSession::new(
        ALLOCATION_ID,
        TUNNEL_ID_A,
        SESSION_ID_1,
        test_mappings(),
        5_000,
        15_000,
        30_000,
        SessionLimits {
            max_tcp_flows: 16,
            max_udp_flows: 16,
        },
        1_000,
    );

    // Setup active TCP and UDP flows before network packet loss.
    let guest_ip = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 30));
    let guest_udp_addr = SocketAddr::new(guest_ip, 49152);

    let _ = relay_session.tcp.begin_tcp_open(0, guest_ip)?;
    let _ = relay_session.tcp.complete_tcp_open(0x00)?;
    let _ = engine_session
        .tcp
        .evaluate_open_header(&TcpOpenHeader { mapping_index: 0 }.encode())
        .map_err(|(_, e)| e)?;

    assert_eq!(relay_session.tcp.metrics().active_tcp_flows, 1);
    assert_eq!(engine_session.tcp.metrics().active_tcp_flows, 1);

    // Send UDP packet through tunnel to register UDP flow.
    let framed_datagram = relay_session
        .udp
        .handle_guest_datagram(1, guest_udp_addr, b"game-packet-1", 1_500)
        .ok_or("missing framed UDP")?;
    engine_session
        .udp
        .handle_relay_datagram(&framed_datagram, 1_510)
        .map_err(|e| format!("engine udp error: {e:?}"))?;

    assert_eq!(relay_session.udp.metrics().active_udp_flows, 1);
    assert_eq!(engine_session.udp.metrics().active_udp_flows, 1);

    // Simulate 100% loss of heartbeat pongs.
    // At t = 11_000 ms (10_000 ms since last activity at 1_000 ms >= 2 * 5_000 ms heartbeat interval):
    // Engine detects lost heartbeats and transitions to Degraded.
    assert_eq!(
        engine_session.evaluate_liveness(11_000),
        EngineSessionState::Degraded
    );

    // At t = 16_000 ms (15_000 ms since last activity >= idle_timeout_ms):
    // Engine detects idle timeout and transitions to Reconnecting.
    assert_eq!(
        engine_session.evaluate_liveness(16_000),
        EngineSessionState::Reconnecting
    );

    // Relay detects idle timeout and enters DisconnectedInGrace.
    let relay_state = relay_session.check_liveness(16_000).clone();
    assert_eq!(
        relay_state,
        RelaySessionConnectivity::DisconnectedInGrace {
            dropped_at_ms: 16_000,
            grace_deadline_ms: 46_000,
        }
    );

    // Invariant: TCP flows are closed immediately upon disconnect (FR-014).
    assert_eq!(engine_session.tcp.metrics().active_tcp_flows, 0);
    assert_eq!(relay_session.tcp.metrics().active_tcp_flows, 0);

    // Invariant: UDP flow tables and socket bindings are preserved for reconnection (NET-004).
    assert_eq!(engine_session.udp.metrics().active_udp_flows, 1);
    assert_eq!(relay_session.udp.metrics().active_udp_flows, 1);

    Ok(())
}

// ---------------------------------------------------------------------------
// 3. Blocked UDP Transport Test (TEST-003, NET-007)
// ---------------------------------------------------------------------------

#[test]
fn test_blocked_udp_transport_keeps_tcp_healthy_without_resource_leaks() -> TestResult {
    let mut engine_session = EngineTunnelSession::new(
        vec![
            LocalMappingConfig::new(
                0,
                "minecraft-java",
                Protocol::Tcp,
                true,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 25565),
            ),
            LocalMappingConfig::new(
                1,
                "minecraft-bedrock",
                Protocol::Udp,
                true,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 19132),
            ),
        ],
        8,
        8,
    )?;

    assert_eq!(engine_session.start()?, EngineSessionState::Connecting);
    assert_eq!(
        engine_session.on_quic_established()?,
        EngineSessionState::Authenticating
    );
    assert_eq!(
        engine_session.on_hello_ack(sample_ack(SESSION_ID_1), 1_000)?,
        EngineSessionState::Active
    );

    let mut relay_session = RelayTunnelSession::new(
        ALLOCATION_ID,
        TUNNEL_ID_A,
        SESSION_ID_1,
        test_mappings(),
        5_000,
        15_000,
        30_000,
        SessionLimits {
            max_tcp_flows: 8,
            max_udp_flows: 8,
        },
        1_000,
    );

    // In blocked-UDP environment:
    // TCP flow establishes successfully.
    let guest_ip = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 45));
    let _ = relay_session.tcp.begin_tcp_open(0, guest_ip)?;
    let _ = relay_session.tcp.complete_tcp_open(0x00)?;
    let _ = engine_session
        .tcp
        .evaluate_open_header(&TcpOpenHeader { mapping_index: 0 }.encode())
        .map_err(|(_, e)| e)?;

    assert_eq!(relay_session.tcp.metrics().active_tcp_flows, 1);
    assert_eq!(engine_session.tcp.metrics().active_tcp_flows, 1);

    // Multiple UDP datagrams are transmitted but filtered/dropped by the impaired network.
    let guest_udp = SocketAddr::new(guest_ip, 50000);
    for i in 0..10 {
        let _payload = format!("blocked-packet-{i}").into_bytes();
        let _ = relay_session
            .udp
            .handle_guest_datagram(1, guest_udp, &_payload, 2_000 + i * 100);
    }

    // Engine UDP receives 0 bytes because UDP is blocked.
    // Engine polls local replies: 0 datagrams sent.
    let engine_replies = engine_session.udp.poll_local_replies(10, 3_000);
    assert!(engine_replies.is_empty());

    // Advance time past UDP flow expiry (default 60_000 ms).
    relay_session.udp.expire_idle_flows(70_000);
    engine_session.udp.expire_idle_flows(70_000);

    // UDP flows are swept cleanly with zero active leaks.
    assert_eq!(relay_session.udp.metrics().active_udp_flows, 0);
    assert_eq!(engine_session.udp.metrics().active_udp_flows, 0);

    // TCP transport and session control stay healthy and Active!
    assert_eq!(engine_session.state(), EngineSessionState::Active);
    assert_eq!(relay_session.tcp.metrics().active_tcp_flows, 1);
    assert_eq!(engine_session.tcp.metrics().active_tcp_flows, 1);

    Ok(())
}

// ---------------------------------------------------------------------------
// 4. Outage Resumption within Grace vs Expiry (NET-004, NET-005, NET-008)
// ---------------------------------------------------------------------------

#[test]
fn test_session_resumption_within_grace_vs_grace_period_expiry() -> TestResult {
    let local_server = UdpSocket::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    local_server.set_read_timeout(Some(Duration::from_millis(500)))?;
    let local_server_addr = local_server.local_addr()?;

    let mut engine = EngineTunnelSession::new(
        vec![
            LocalMappingConfig::new(
                0,
                "minecraft-java",
                Protocol::Tcp,
                true,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 25565),
            ),
            LocalMappingConfig::new(
                1,
                "minecraft-bedrock",
                Protocol::Udp,
                true,
                local_server_addr,
            ),
        ],
        8,
        8,
    )?;

    assert_eq!(engine.start()?, EngineSessionState::Connecting);
    assert_eq!(
        engine.on_quic_established()?,
        EngineSessionState::Authenticating
    );
    assert_eq!(
        engine.on_hello_ack(sample_ack(SESSION_ID_1), 1_000)?,
        EngineSessionState::Active
    );

    let mut relay = RelayTunnelSession::new(
        ALLOCATION_ID,
        TUNNEL_ID_A,
        SESSION_ID_1,
        test_mappings(),
        5_000,
        15_000,
        30_000, // 30 second grace period
        SessionLimits {
            max_tcp_flows: 8,
            max_udp_flows: 8,
        },
        1_000,
    );

    // Bind UDP flow 99 before outage.
    let guest_udp = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 50)), 33333);
    let _ = relay
        .udp
        .handle_guest_datagram(1, guest_udp, b"before-outage", 2_000);
    let encoded_pkt = encode_udp_datagram(1, 99, b"before-outage", DEFAULT_MAX_DATAGRAM_BYTES)?;
    engine
        .udp
        .handle_relay_datagram(&encoded_pkt, 2_010)
        .map_err(|e| format!("engine udp error: {e:?}"))?;

    let bound_engine_flow_addr = engine
        .udp
        .flow_local_addr(1, 99)
        .ok_or("missing bound engine flow socket")?;

    // Drop occurs at t = 10_000 ms.
    let _ = engine.on_connection_lost()?;
    assert_eq!(engine.state(), EngineSessionState::Reconnecting);
    let _ = relay.on_transport_disconnected(10_000);

    assert_eq!(
        relay.connectivity(),
        &RelaySessionConnectivity::DisconnectedInGrace {
            dropped_at_ms: 10_000,
            grace_deadline_ms: 40_000,
        }
    );

    // Part A: Resumption succeeds within grace period (at t = 22_000 ms < 40_000 ms deadline).
    assert!(relay.resume(SESSION_ID_2, 22_000).is_ok());
    assert_eq!(
        relay.connectivity(),
        &RelaySessionConnectivity::Active {
            session_id: SESSION_ID_2.to_owned()
        }
    );

    let ack_resumed = sample_ack(SESSION_ID_2);
    assert_eq!(
        engine.on_hello_ack(ack_resumed.clone(), 22_000)?,
        EngineSessionState::Active
    );
    assert_eq!(engine.session_id(), Some(SESSION_ID_2));

    // Public ports are identical.
    assert_eq!(relay.mappings(), test_mappings().as_slice());
    assert_eq!(engine.negotiated_mappings(), test_mappings().as_slice());

    // Resumed UDP flow reuses the exact same bound socket without port churn.
    let resumed_pkt = encode_udp_datagram(1, 99, b"after-resume", DEFAULT_MAX_DATAGRAM_BYTES)?;
    engine
        .udp
        .handle_relay_datagram(&resumed_pkt, 22_100)
        .map_err(|e| format!("resumed engine udp error: {e:?}"))?;

    assert_eq!(
        engine.udp.flow_local_addr(1, 99),
        Some(bound_engine_flow_addr)
    );

    // Part B: Outage exceeding grace period.
    // Another drop occurs at t = 30_000 ms (grace deadline = 60_000 ms).
    let _ = relay.on_transport_disconnected(30_000);

    // Time advances past grace deadline (at t = 65_000 ms > 60_000 ms).
    let expired_state = relay.check_liveness(65_000).clone();
    assert_eq!(expired_state, RelaySessionConnectivity::Released);

    // Attempting to resume an expired session fails.
    assert!(relay.resume("resumed-too-late-session", 65_000).is_err());

    Ok(())
}

// ---------------------------------------------------------------------------
// 5. IPv4 and IPv6 Dual-Stack and Address Mismatch Scenarios (NET-010, FR-008, E-13)
// ---------------------------------------------------------------------------

#[test]
fn test_ipv4_and_ipv6_dual_stack_and_mismatch_scenarios() -> TestResult {
    let mut enforcer = RelayAdmissionEnforcer::new(1_024);

    let now_ms = 1_700_000_000_000u64;
    let expires_ms = now_ms + 86_400_000;

    let ipv4_addr = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 77));
    let ipv6_addr = IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0xcafe, 0xbabe));
    let mapped_ipv6 = IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0xc633, 0x644d)); // ::ffff:198.51.100.77

    // --- Subcase 5.1: Native IPv4 Happy Path ---
    let _ = enforcer.apply_state_sync(
        false,
        &[RelayAdmissionSyncDelta {
            admission_id: "adm-v4-happy".to_owned(),
            tunnel_id: TUNNEL_ID_A.to_owned(),
            source_ip: ipv4_addr,
            active: true,
            expires_at_epoch_ms: expires_ms,
            revoked_at_epoch_ms: None,
        }],
        now_ms,
    );
    assert!(enforcer.is_guest_admitted(TUNNEL_ID_A, ipv4_addr, now_ms));

    // --- Subcase 5.2: Native IPv6 Happy Path ---
    let _ = enforcer.apply_state_sync(
        false,
        &[RelayAdmissionSyncDelta {
            admission_id: "adm-v6-happy".to_owned(),
            tunnel_id: TUNNEL_ID_A.to_owned(),
            source_ip: ipv6_addr,
            active: true,
            expires_at_epoch_ms: expires_ms,
            revoked_at_epoch_ms: None,
        }],
        now_ms,
    );
    assert!(enforcer.is_guest_admitted(TUNNEL_ID_A, ipv6_addr, now_ms));

    // --- Subcase 5.3: Dual-Stack IPv4-Mapped IPv6 Normalization ---
    // canonicalize_guest_ip normalizes ::ffff:198.51.100.77 to 198.51.100.77.
    assert_eq!(canonicalize_guest_ip(mapped_ipv6), ipv4_addr);
    // When dual-stack socket receives connection with mapped IPv6, enforcer accepts it.
    assert!(enforcer.is_guest_admitted(TUNNEL_ID_A, mapped_ipv6, now_ms));

    // --- Subcase 5.4: Documented Mismatch Case 1: Claim IPv6, Game connects over IPv4 (NET-010, E-13) ---
    // Guest claims link on mobile browser over IPv6 (2001:db8::dead:beef).
    let claim_only_v6 = IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0xdead, 0xbeef));
    let game_only_v4 = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 99));

    let _ = enforcer.apply_state_sync(
        false,
        &[RelayAdmissionSyncDelta {
            admission_id: "adm-v6-claim-only".to_owned(),
            tunnel_id: TUNNEL_ID_A.to_owned(),
            source_ip: claim_only_v6,
            active: true,
            expires_at_epoch_ms: expires_ms,
            revoked_at_epoch_ms: None,
        }],
        now_ms,
    );

    // The game connects from game_only_v4:
    // Observed and Documented Behavior: Default-deny refuses unadmitted IPv4 connection!
    assert!(!enforcer.is_guest_admitted(TUNNEL_ID_A, game_only_v4, now_ms));

    // --- Subcase 5.5: Documented Mismatch Case 2: Claim IPv4, Game connects over IPv6 (NET-010, E-13) ---
    // Guest claims link on browser over IPv4 (203.0.113.100), game client uses IPv6 (2001:db8::1111).
    let claim_only_v4 = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 100));
    let game_only_v6 = IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0x1111, 0x1111));

    let _ = enforcer.apply_state_sync(
        false,
        &[RelayAdmissionSyncDelta {
            admission_id: "adm-v4-claim-only".to_owned(),
            tunnel_id: TUNNEL_ID_A.to_owned(),
            source_ip: claim_only_v4,
            active: true,
            expires_at_epoch_ms: expires_ms,
            revoked_at_epoch_ms: None,
        }],
        now_ms,
    );

    // Observed and Documented Behavior: Default-deny refuses unadmitted IPv6 connection!
    assert!(!enforcer.is_guest_admitted(TUNNEL_ID_A, game_only_v6, now_ms));

    // --- Subcase 5.6: Mobile / CGNAT Address Hopping ---
    // Guest IP changes from WiFi (198.51.100.10) to Cellular (203.0.113.88).
    let wifi_ip = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 10));
    let cellular_ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 88));

    let _ = enforcer.apply_state_sync(
        false,
        &[RelayAdmissionSyncDelta {
            admission_id: "adm-wifi".to_owned(),
            tunnel_id: TUNNEL_ID_A.to_owned(),
            source_ip: wifi_ip,
            active: true,
            expires_at_epoch_ms: expires_ms,
            revoked_at_epoch_ms: None,
        }],
        now_ms,
    );
    assert!(enforcer.is_guest_admitted(TUNNEL_ID_A, wifi_ip, now_ms));
    assert!(!enforcer.is_guest_admitted(TUNNEL_ID_A, cellular_ip, now_ms));

    // Guest re-claims link on Cellular browser -> delta sync adds cellular_ip.
    let _ = enforcer.apply_state_sync(
        false,
        &[RelayAdmissionSyncDelta {
            admission_id: "adm-cellular".to_owned(),
            tunnel_id: TUNNEL_ID_A.to_owned(),
            source_ip: cellular_ip,
            active: true,
            expires_at_epoch_ms: expires_ms,
            revoked_at_epoch_ms: None,
        }],
        now_ms + 10_000,
    );
    assert!(enforcer.is_guest_admitted(TUNNEL_ID_A, cellular_ip, now_ms + 10_000));

    // --- Subcase 5.7: Tenant Isolation (SEC-009) ---
    // Admission on Tunnel A must NEVER admit traffic on Tunnel B.
    assert!(enforcer.is_guest_admitted(TUNNEL_ID_A, ipv4_addr, now_ms));
    assert!(!enforcer.is_guest_admitted(TUNNEL_ID_B, ipv4_addr, now_ms));

    Ok(())
}
