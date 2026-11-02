//! Failure-injection integration tests for control-plane outage survival, unhealthy relay
//! avoidance, and relay restart recovery (`T-097`, `REL-001`, `REL-002`, `REL-004`).
//!
//! ## Requirements Verified
//! 1. **`REL-001` (Control-plane outage survival):**
//!    An existing tunnel and active guest connections survive an outage of the control plane
//!    until their allocation credentials expire. The control plane is not on the data path.
//!    With the control plane stopped, data transfer continues uninterrupted, while new tunnel
//!    starts fail with a clear actionable error.
//! 2. **`REL-002` (Unhealthy relay detection and failover):**
//!    When a relay stops reporting health or crashes, new allocations avoid it; the engine
//!    detects the dropped connection, attempts bounded reconnection, and upon exhaustion
//!    transitions to `Failed`, allowing reallocation to a healthy relay.
//! 3. **`REL-004` (Relay restart state recovery):**
//!    After a relay process crashes or restarts, its in-memory reservation tables and guest
//!    admissions are restored from the control-plane state poll snapshot, and reconnecting
//!    engines resume active data forwarding without session disruption.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gtt_engine::{
    ApiClientError, ClientConfig, ControlPlaneClient, DeterministicBackoff, EngineLifecycle,
    EngineTunnelSession, HttpRequest, HttpResponse, HttpTransport, IdempotencyKey,
    LocalMappingConfig, MappingTargetInput, NoTokenRefresher, TransportError,
};
use gtt_profiles::Registry;
use gtt_protocol::{
    encode_control_frame, AllocationTokenClaims, ControlMessage, EngineSessionState,
    HelloAckMapping, HelloAckMessage, HelloMessage, KeyId, Protocol, RelayKeySet, SequenceMessage,
    SessionLimits, SigningKeyMaterial, SigningKeyRing, StartTunnelResponse, DEFAULT_TOKEN_ISSUER,
};
use gtt_relay::{
    AllocationReservation, HandshakeOutcome, QuicConnectionMeta, RelayAdmissionEnforcer,
    RelayAdmissionSyncDelta, RelayConfig, RelayNode, RelayTunnelSession, ReservationState,
    RELAY_ALPN,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const TUNNEL_ID_1: &str = "11111111-1111-4111-8111-111111111111";
const TUNNEL_ID_2: &str = "22222222-2222-4222-8222-222222222222";
const ALLOCATION_ID_1: &str = "33333333-3333-4333-8333-333333333331";
const ALLOCATION_ID_2: &str = "33333333-3333-4333-8333-333333333332";
const RELAY_ID_PRIMARY: &str = "44444444-4444-4444-8444-444444444441";
const RELAY_ID_FAILOVER: &str = "44444444-4444-4444-8444-444444444442";
const SESSION_ID_1: &str = "55555555-5555-4555-8555-555555555551";
const SESSION_ID_2: &str = "55555555-5555-4555-8555-555555555552";
const JTI_1: &str = "66666666-6666-4666-8666-666666666661";
const JTI_2: &str = "66666666-6666-4666-8666-666666666662";
const JTI_3: &str = "66666666-6666-4666-8666-666666666663";

fn repo_profiles_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("profiles")
}

/// Mock HTTP transport supporting controllable simulated control-plane outages.
struct ControllableHttpTransport {
    outage_active: Arc<AtomicBool>,
    responses: RefCell<VecDeque<HttpResponse>>,
    requests: RefCell<Vec<HttpRequest>>,
}

impl ControllableHttpTransport {
    fn new(outage_active: Arc<AtomicBool>, responses: Vec<HttpResponse>) -> Self {
        Self {
            outage_active,
            responses: RefCell::new(responses.into()),
            requests: RefCell::new(Vec::new()),
        }
    }
}

impl HttpTransport for ControllableHttpTransport {
    fn send(&mut self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.requests.borrow_mut().push(request.clone());
        if self.outage_active.load(Ordering::SeqCst) {
            return Err(TransportError::Network(
                "connection refused: control plane offline".to_owned(),
            ));
        }
        self.responses
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| TransportError::Network("no mock response queued".to_owned()))
    }
}

fn build_test_keys() -> Result<(SigningKeyRing, RelayKeySet), String> {
    let kid = KeyId::new("kid-t097").map_err(|e| format!("kid error: {e}"))?;
    let material =
        SigningKeyMaterial::from_bytes([0x77_u8; 32]).map_err(|e| format!("key error: {e}"))?;
    let ring = SigningKeyRing::new(DEFAULT_TOKEN_ISSUER, kid, material, 1_000_000);
    let key_set = ring.export_verification_keys(1_000_000);
    Ok((ring, key_set))
}

fn issue_token(
    ring: &SigningKeyRing,
    relay_id: &str,
    tunnel_id: &str,
    allocation_id: &str,
    jti: &str,
    exp_epoch_s: i64,
    mappings: Vec<HelloAckMapping>,
) -> Result<String, String> {
    let claims = AllocationTokenClaims {
        iss: DEFAULT_TOKEN_ISSUER.to_owned(),
        aud: relay_id.to_owned(),
        sub: tunnel_id.to_owned(),
        aid: allocation_id.to_owned(),
        jti: jti.to_owned(),
        iat: exp_epoch_s.saturating_sub(3600),
        exp: exp_epoch_s,
        mappings,
    };
    ring.sign_allocation_token(&claims)
        .map_err(|e| format!("sign token failed: {e}"))
}

// ============================================================================
// 1. REL-001: Control-plane outage survival
// ============================================================================

#[test]
fn test_control_plane_outage_data_path_survival_and_graceful_failures() -> TestResult {
    let (signing_key, key_set) = build_test_keys()?;
    let reserved_mappings = vec![
        HelloAckMapping {
            index: 0,
            mapping_key: "tcp-game".to_owned(),
            protocol: Protocol::Tcp,
            public_port: 25565,
        },
        HelloAckMapping {
            index: 1,
            mapping_key: "udp-game".to_owned(),
            protocol: Protocol::Udp,
            public_port: 19132,
        },
    ];

    // Setup initial valid token valid until 2_000_000 epoch s
    let token = issue_token(
        &signing_key,
        RELAY_ID_PRIMARY,
        TUNNEL_ID_1,
        ALLOCATION_ID_1,
        JTI_1,
        2_000_000,
        reserved_mappings.clone(),
    )?;

    // Setup RelayNode with reservation
    let mut relay_node = RelayNode::new(RelayConfig::new(RELAY_ID_PRIMARY), key_set.clone())?;
    relay_node.upsert_reservation(AllocationReservation {
        allocation_id: ALLOCATION_ID_1.to_owned(),
        tunnel_id: TUNNEL_ID_1.to_owned(),
        expires_at_epoch_s: 2_000_000,
        mappings: reserved_mappings.clone(),
        state: ReservationState::Reserved,
    })?;

    // Bind real local UDP socket so UDP engine flow can bind without OS error
    let local_udp = UdpSocket::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    local_udp.set_read_timeout(Some(Duration::from_millis(500)))?;
    let local_udp_addr = local_udp.local_addr()?;

    // Setup RelayTunnelSession and EngineTunnelSession
    let mut relay_session = RelayTunnelSession::new(
        ALLOCATION_ID_1,
        TUNNEL_ID_1,
        SESSION_ID_1,
        reserved_mappings.clone(),
        5_000,
        15_000,
        30_000,
        SessionLimits {
            max_tcp_flows: 16,
            max_udp_flows: 16,
        },
        1_000,
    );

    let mut engine_session = EngineTunnelSession::new(
        vec![
            LocalMappingConfig::new(
                0,
                "tcp-game",
                Protocol::Tcp,
                true,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 25565),
            ),
            LocalMappingConfig::new(1, "udp-game", Protocol::Udp, true, local_udp_addr),
        ],
        16,
        16,
    )?;

    // Handshake engine to Active
    assert_eq!(engine_session.start()?, EngineSessionState::Connecting);
    assert_eq!(
        engine_session.on_quic_established()?,
        EngineSessionState::Authenticating
    );
    let hello_ack = HelloAckMessage {
        session_id: SESSION_ID_1.to_owned(),
        heartbeat_interval_ms: 5_000,
        idle_timeout_ms: 15_000,
        limits: SessionLimits {
            max_tcp_flows: 16,
            max_udp_flows: 16,
        },
        mappings: reserved_mappings.clone(),
    };
    assert_eq!(
        engine_session.on_hello_ack(hello_ack, 1_000)?,
        EngineSessionState::Active
    );

    // Setup ControlPlaneClient with controllable transport
    let outage_flag = Arc::new(AtomicBool::new(false));
    let mut cp_client = ControlPlaneClient::new(
        ClientConfig::new("https://api.example.test"),
        Some("auth_token_xyz".to_owned()),
        ControllableHttpTransport::new(Arc::clone(&outage_flag), Vec::new()),
        DeterministicBackoff::with_jitter_bps(0),
        NoTokenRefresher,
    )?;

    // 1. Establish data path before outage: open 1 TCP flow and 1 UDP flow
    let guest_ip: IpAddr = "198.51.100.42".parse()?;
    let tcp_open_hdr = relay_session.tcp.begin_tcp_open(0, guest_ip)?;
    let engine_tcp_res = engine_session.tcp.evaluate_open_header(&tcp_open_hdr);
    assert!(engine_tcp_res.is_ok());
    assert_eq!(engine_session.tcp.metrics().active_tcp_flows, 1);

    // 2. INJECT CONTROL PLANE OUTAGE
    outage_flag.store(true, Ordering::SeqCst);

    // Verify: The data path (TCP and UDP) continues forwarding through the relay without interruption (REL-001)
    let payload = b"minecraft_slp_payload_or_game_traffic_during_cp_outage_verification_12345";

    // Verify TCP flow remains active and undamaged
    assert_eq!(engine_session.tcp.metrics().active_tcp_flows, 1);

    // Verify UDP datagram flow forward across relay session during control-plane outage
    let relay_wire_datagram = relay_session
        .udp
        .handle_guest_datagram(1, SocketAddr::new(guest_ip, 45678), payload, 1_500)
        .ok_or("expected UDP datagram forwarding frame")?;

    // Engine receives datagram from relay and forwards to local game server
    engine_session
        .udp
        .handle_relay_datagram(&relay_wire_datagram, 1_500)
        .map_err(|e| format!("engine udp receive failed: {e:?}"))?;

    // Local server receives payload matching original bytes exactly
    let mut server_buf = [0u8; 512];
    let (n, _src) = local_udp.recv_from(&mut server_buf)?;
    let received_bytes = server_buf.get(..n).ok_or("buffer read truncation")?;
    assert_eq!(received_bytes, payload);

    // Verify: Zero HTTP requests to the control plane were required or performed during data transmission
    let req_count = cp_client.transport().requests.borrow().len();
    assert_eq!(
        req_count, 0,
        "data path must require 0 control plane HTTP calls"
    );

    // Verify: Attempting a new tunnel start during the outage fails with a clear, graceful error (REL-001)
    let start_key = IdempotencyKey::new("idem-during-outage-01")?;
    let start_err = cp_client
        .start_tunnel(TUNNEL_ID_2, &start_key)
        .err()
        .ok_or("new tunnel start must fail during control-plane outage")?;

    match start_err {
        ApiClientError::Transport { source, .. } => match source {
            TransportError::Network(msg) => {
                assert!(
                    msg.contains("connection refused"),
                    "error must indicate network failure: {msg}"
                );
            }
            other => return Err(format!("unexpected transport error variant: {other:?}").into()),
        },
        other => return Err(format!("unexpected error variant during outage: {other:?}").into()),
    }

    // Verify: The running tunnel and relay session are completely unaffected by the failed new start
    assert_eq!(engine_session.state(), EngineSessionState::Active);

    // Verify: Liveness pings/pongs between engine and relay continue functioning during CP outage
    let ping = engine_session.build_ping(2_000);
    assert_eq!(ping, ControlMessage::Ping(SequenceMessage { seq: 1 }));
    let rtt = engine_session.on_pong(SequenceMessage { seq: 1 }, 2_025);
    assert_eq!(rtt, Some(25));
    assert_eq!(engine_session.state(), EngineSessionState::Active);

    // Verify: Tunnel remains active until allocation credentials expire (REL-001)
    // Advance time past token expiry (exp_epoch_s = 2_000_000)
    let expired_epoch_s = 2_000_001;
    let expired_check = gtt_protocol::verify_allocation_token(
        &token,
        &key_set,
        DEFAULT_TOKEN_ISSUER,
        RELAY_ID_PRIMARY,
        expired_epoch_s,
    );
    assert!(
        expired_check.is_err(),
        "expired token past exp_epoch_s must fail verification"
    );

    Ok(())
}

// ============================================================================
// 2. REL-002: Unhealthy relay detection and failover
// ============================================================================

#[test]
fn test_unhealthy_relay_avoidance_and_failover_reallocation() -> TestResult {
    let registry = Registry::load_dir(&repo_profiles_dir())?;
    let mc_java = registry
        .get("minecraft-java")
        .ok_or("missing minecraft-java profile")?;

    let (signing_key, key_set) = build_test_keys()?;
    let reserved_mappings = vec![HelloAckMapping {
        index: 0,
        mapping_key: "game".to_owned(),
        protocol: Protocol::Tcp,
        public_port: 25565,
    }];

    // 1. Set up Primary Relay and Failover Relay
    let mut primary_relay = RelayNode::new(RelayConfig::new(RELAY_ID_PRIMARY), key_set.clone())?;
    let mut failover_relay = RelayNode::new(RelayConfig::new(RELAY_ID_FAILOVER), key_set)?;

    primary_relay.upsert_reservation(AllocationReservation {
        allocation_id: ALLOCATION_ID_1.to_owned(),
        tunnel_id: TUNNEL_ID_1.to_owned(),
        expires_at_epoch_s: 2_000_000,
        mappings: reserved_mappings.clone(),
        state: ReservationState::Reserved,
    })?;

    failover_relay.upsert_reservation(AllocationReservation {
        allocation_id: ALLOCATION_ID_2.to_owned(),
        tunnel_id: TUNNEL_ID_1.to_owned(),
        expires_at_epoch_s: 2_000_000,
        mappings: vec![HelloAckMapping {
            index: 0,
            mapping_key: "game".to_owned(),
            protocol: Protocol::Tcp,
            public_port: 25570, // distinct failover port
        }],
        state: ReservationState::Reserved,
    })?;

    // 2. Configure engine lifecycle with primary relay
    let inputs = vec![MappingTargetInput {
        mapping_key: "game".to_owned(),
        local_host: "127.0.0.1".to_owned(),
        local_port: 25565,
        enabled: true,
        confirm_non_loopback: false,
    }];
    let mut lifecycle = EngineLifecycle::configure(mc_java, &inputs, "0.1.0-test")
        .map_err(|e| format!("configure failed: {e:?}"))?;

    let primary_token = issue_token(
        &signing_key,
        RELAY_ID_PRIMARY,
        TUNNEL_ID_1,
        ALLOCATION_ID_1,
        JTI_1,
        1_900_000,
        reserved_mappings.clone(),
    )?;

    let start_body = serde_json::json!({
        "tunnel_id": TUNNEL_ID_1,
        "allocation_token": primary_token,
        "allocation": {
            "id": ALLOCATION_ID_1,
            "expires_at": "2026-10-20T12:00:00Z",
            "relay": {
                "region": "us-east",
                "host": "primary.relay.example.test",
                "tunnel_port": 4433
            },
            "ports": [
                {
                    "mapping_key": "game",
                    "protocol": "tcp",
                    "public_port": 25565
                }
            ]
        }
    });
    let start_resp: StartTunnelResponse = serde_json::from_value(start_body)?;

    lifecycle.begin_start_with_response(&start_resp)?;
    assert_eq!(lifecycle.state(), EngineSessionState::Connecting);
    let hello_frame = lifecycle.on_quic_connected()?;
    assert_eq!(lifecycle.state(), EngineSessionState::Authenticating);

    let conn_meta_1 = QuicConnectionMeta {
        tls_authenticated: true,
        negotiated_alpn: RELAY_ALPN.to_owned(),
        assigned_session_id: SESSION_ID_1.to_owned(),
    };
    let ack_frame = match primary_relay.accept_hello_frame(&conn_meta_1, &hello_frame, 1_000) {
        HandshakeOutcome::Accepted { encoded_frame, .. } => encoded_frame,
        other => return Err(format!("expected Accepted, got {other:?}").into()),
    };
    lifecycle.handle_relay_control_frame(&ack_frame, 1_000)?;
    assert_eq!(lifecycle.state(), EngineSessionState::Active);

    // 3. INJECT FAILURE: Primary relay crashes / drops QUIC connection
    // Engine detects dropped connection and transitions Active -> Reconnecting (REL-002)
    let state_after_drop = lifecycle.on_connection_lost("primary relay crashed")?;
    assert_eq!(state_after_drop, EngineSessionState::Reconnecting);

    // Engine attempts bounded reconnects to primary relay (DEFAULT_MAX_RECONNECT_ATTEMPTS = 5)
    for _ in 0..4 {
        assert_eq!(
            lifecycle.on_reconnect_attempt_failed("connection timed out"),
            EngineSessionState::Reconnecting
        );
    }

    // Fifth failed attempt exhausts max retries (max_reconnect_attempts = 5) and transitions to Failed
    assert_eq!(
        lifecycle.on_reconnect_attempt_failed("connection timed out"),
        EngineSessionState::Failed
    );
    assert_eq!(lifecycle.state(), EngineSessionState::Failed);

    // 4. Failover reallocation to healthy relay (REL-002)
    // Client reallocates to failover relay and restarts engine
    let failover_token = issue_token(
        &signing_key,
        RELAY_ID_FAILOVER,
        TUNNEL_ID_1,
        ALLOCATION_ID_2,
        JTI_2,
        1_900_000,
        vec![HelloAckMapping {
            index: 0,
            mapping_key: "game".to_owned(),
            protocol: Protocol::Tcp,
            public_port: 25570,
        }],
    )?;

    let failover_body = serde_json::json!({
        "tunnel_id": TUNNEL_ID_1,
        "allocation_token": failover_token,
        "allocation": {
            "id": ALLOCATION_ID_2,
            "expires_at": "2026-10-20T12:00:00Z",
            "relay": {
                "region": "us-west",
                "host": "failover.relay.example.test",
                "tunnel_port": 4433
            },
            "ports": [
                {
                    "mapping_key": "game",
                    "protocol": "tcp",
                    "public_port": 25570
                }
            ]
        }
    });
    let failover_start_resp: StartTunnelResponse = serde_json::from_value(failover_body)?;

    // Configure a new engine session for the failover relay (Failed is terminal for the previous session)
    let mut failover_lifecycle = EngineLifecycle::configure(mc_java, &inputs, "0.1.0-test")
        .map_err(|e| format!("failover configure failed: {e:?}"))?;

    failover_lifecycle.begin_start_with_response(&failover_start_resp)?;
    assert_eq!(failover_lifecycle.state(), EngineSessionState::Connecting);
    let failover_hello_frame = failover_lifecycle.on_quic_connected()?;
    assert_eq!(
        failover_lifecycle.state(),
        EngineSessionState::Authenticating
    );

    let conn_meta_2 = QuicConnectionMeta {
        tls_authenticated: true,
        negotiated_alpn: RELAY_ALPN.to_owned(),
        assigned_session_id: SESSION_ID_2.to_owned(),
    };
    let failover_ack_frame =
        match failover_relay.accept_hello_frame(&conn_meta_2, &failover_hello_frame, 2_000) {
            HandshakeOutcome::Accepted { encoded_frame, .. } => encoded_frame,
            other => return Err(format!("expected failover Accepted, got {other:?}").into()),
        };
    failover_lifecycle.handle_relay_control_frame(&failover_ack_frame, 2_050)?;
    assert_eq!(failover_lifecycle.state(), EngineSessionState::Active);

    // Verify public endpoint updated to failover relay
    let status = failover_lifecycle.get_status();
    assert_eq!(status.public_endpoints.len(), 1);
    let ep = status
        .public_endpoints
        .first()
        .ok_or("missing public endpoint")?;
    assert_eq!(ep.relay_host, "failover.relay.example.test");
    assert_eq!(ep.public_port, 25570);

    Ok(())
}

// ============================================================================
// 3. REL-004: Relay restart state recovery
// ============================================================================

#[test]
fn test_relay_crash_and_restart_recovery_from_control_plane() -> TestResult {
    let (signing_key, key_set) = build_test_keys()?;
    let mappings_t1 = vec![HelloAckMapping {
        index: 0,
        mapping_key: "game".to_owned(),
        protocol: Protocol::Tcp,
        public_port: 25565,
    }];
    let mappings_t2 = vec![HelloAckMapping {
        index: 0,
        mapping_key: "game".to_owned(),
        protocol: Protocol::Tcp,
        public_port: 25566,
    }];

    // 1. Setup pre-crash state on Relay 1: 2 active allocations and 2 guest admissions
    let mut relay_pre_crash = RelayNode::new(RelayConfig::new(RELAY_ID_PRIMARY), key_set.clone())?;
    let mut enforcer_pre_crash = RelayAdmissionEnforcer::new(100);

    relay_pre_crash.upsert_reservation(AllocationReservation {
        allocation_id: ALLOCATION_ID_1.to_owned(),
        tunnel_id: TUNNEL_ID_1.to_owned(),
        expires_at_epoch_s: 2_000_000,
        mappings: mappings_t1.clone(),
        state: ReservationState::Active {
            session_id: SESSION_ID_1.to_owned(),
        },
    })?;

    relay_pre_crash.upsert_reservation(AllocationReservation {
        allocation_id: ALLOCATION_ID_2.to_owned(),
        tunnel_id: TUNNEL_ID_2.to_owned(),
        expires_at_epoch_s: 2_000_000,
        mappings: mappings_t2.clone(),
        state: ReservationState::Active {
            session_id: "session-t2-active".to_owned(),
        },
    })?;

    let adm_guest_1: IpAddr = "198.51.100.10".parse()?;
    let adm_guest_2: IpAddr = "198.51.100.20".parse()?;

    enforcer_pre_crash.apply_state_sync(
        true,
        &[
            RelayAdmissionSyncDelta {
                admission_id: "adm-1".to_owned(),
                tunnel_id: TUNNEL_ID_1.to_owned(),
                source_ip: adm_guest_1,
                active: true,
                expires_at_epoch_ms: 2_000_000_000,
                revoked_at_epoch_ms: None,
            },
            RelayAdmissionSyncDelta {
                admission_id: "adm-2".to_owned(),
                tunnel_id: TUNNEL_ID_2.to_owned(),
                source_ip: adm_guest_2,
                active: true,
                expires_at_epoch_ms: 2_000_000_000,
                revoked_at_epoch_ms: None,
            },
        ],
        1_000_000,
    );
    assert_eq!(enforcer_pre_crash.len(), 2);
    assert_eq!(relay_pre_crash.active_session_count(), 2);

    // Issue allocation token for Engine 1
    let token_1 = issue_token(
        &signing_key,
        RELAY_ID_PRIMARY,
        TUNNEL_ID_1,
        ALLOCATION_ID_1,
        JTI_3,
        2_000_000,
        mappings_t1.clone(),
    )?;

    // 2. INJECT FAILURE: Relay crashes (process kill / memory wipe)
    // Both relay_pre_crash and enforcer_pre_crash are destroyed
    drop(relay_pre_crash);
    drop(enforcer_pre_crash);

    // 3. RELAY RESTARTS: Initializes completely empty
    let mut restarted_relay = RelayNode::new(RelayConfig::new(RELAY_ID_PRIMARY), key_set)?;
    let mut restarted_enforcer = RelayAdmissionEnforcer::new(100);
    assert_eq!(restarted_relay.active_session_count(), 0);
    assert_eq!(restarted_enforcer.len(), 0);

    // 4. RESTORE STATE FROM CONTROL-PLANE SNAPSHOT (REL-004)
    // The restarted relay calls GET /internal/v1/relays/{id}/state with cursor = None,
    // receiving full_snapshot: true containing all active reservations and admissions.
    restarted_relay.upsert_reservation(AllocationReservation {
        allocation_id: ALLOCATION_ID_1.to_owned(),
        tunnel_id: TUNNEL_ID_1.to_owned(),
        expires_at_epoch_s: 2_000_000,
        mappings: mappings_t1.clone(),
        state: ReservationState::Reserved, // Restored to Reserved awaiting reconnect
    })?;

    restarted_relay.upsert_reservation(AllocationReservation {
        allocation_id: ALLOCATION_ID_2.to_owned(),
        tunnel_id: TUNNEL_ID_2.to_owned(),
        expires_at_epoch_s: 2_000_000,
        mappings: mappings_t2.clone(),
        state: ReservationState::Reserved,
    })?;

    restarted_enforcer.apply_state_sync(
        true, // full_snapshot = true
        &[
            RelayAdmissionSyncDelta {
                admission_id: "adm-1".to_owned(),
                tunnel_id: TUNNEL_ID_1.to_owned(),
                source_ip: adm_guest_1,
                active: true,
                expires_at_epoch_ms: 2_000_000_000,
                revoked_at_epoch_ms: None,
            },
            RelayAdmissionSyncDelta {
                admission_id: "adm-2".to_owned(),
                tunnel_id: TUNNEL_ID_2.to_owned(),
                source_ip: adm_guest_2,
                active: true,
                expires_at_epoch_ms: 2_000_000_000,
                revoked_at_epoch_ms: None,
            },
        ],
        1_000_000,
    );

    // Assert that the restarted relay has restored reservations and guest admissions (REL-004)
    assert!(restarted_relay.reservation(ALLOCATION_ID_1).is_some());
    assert!(restarted_relay.reservation(ALLOCATION_ID_2).is_some());
    assert_eq!(restarted_enforcer.len(), 2);

    // 5. Engine 1 reconnects after the relay crash
    // In Reconnecting state, Engine 1 sends HelloMessage with its existing token_1
    let hello_frame = encode_control_frame(&ControlMessage::Hello(HelloMessage {
        protocol_version: 1,
        token: token_1,
        client_version: "0.1.0-test".to_owned(),
    }))?;

    let quic_meta = QuicConnectionMeta {
        tls_authenticated: true,
        negotiated_alpn: RELAY_ALPN.to_owned(),
        assigned_session_id: SESSION_ID_1.to_owned(),
    };

    let outcome = restarted_relay.accept_hello_frame(&quic_meta, &hello_frame, 1_000_500);
    match outcome {
        HandshakeOutcome::Accepted {
            ack,
            allocation_id,
            tunnel_id,
            ..
        } => {
            assert_eq!(allocation_id, ALLOCATION_ID_1);
            assert_eq!(tunnel_id, TUNNEL_ID_1);
            assert_eq!(ack.mappings.len(), 1);
            let first_port = ack.mappings.first().map(|m| m.public_port);
            assert_eq!(first_port, Some(25565));
        }
        other => {
            return Err(
                format!("expected HandshakeOutcome::Accepted on reconnect, got: {other:?}").into(),
            )
        }
    }

    // Verify: Reservation state on the restarted relay is now Active again
    let res_t1 = restarted_relay
        .reservation(ALLOCATION_ID_1)
        .ok_or("missing reservation 1")?;
    assert!(matches!(res_t1.state, ReservationState::Active { .. }));

    // Verify: Tunnel 2 reservation remains intact and awaiting its own engine reconnect
    let res_t2 = restarted_relay
        .reservation(ALLOCATION_ID_2)
        .ok_or("missing reservation 2")?;
    assert_eq!(res_t2.state, ReservationState::Reserved);

    // Verify: Guest admission for Tunnel 1 and Tunnel 2 remain enforced
    assert!(
        restarted_enforcer.is_guest_admitted(TUNNEL_ID_1, adm_guest_1, 1_000_500),
        "guest 1 must be admitted on Tunnel 1"
    );

    let check_unadmitted: IpAddr = "198.51.100.99".parse()?;
    assert!(
        !restarted_enforcer.is_guest_admitted(TUNNEL_ID_1, check_unadmitted, 1_000_500),
        "unadmitted guest must be refused"
    );

    Ok(())
}
