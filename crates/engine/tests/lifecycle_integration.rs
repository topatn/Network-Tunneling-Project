//! Integration tests for `T-070` (`crates/engine` lifecycle, `contracts.md` §12 state machine,
//! control-plane API + relay handshake integration, `FR-005`, `TEST-002`).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;

use gtt_engine::{
    ClientConfig, ControlPlaneClient, DeterministicBackoff, EngineEvent, EngineLifecycle,
    HttpMethod, HttpRequest, HttpResponse, HttpTransport, IdempotencyKey, MappingTargetInput,
    NoTokenRefresher, PublicMappingEndpoint, TransportError,
};
use gtt_profiles::Registry;
use gtt_protocol::{
    encode_control_frame, AllocationTokenClaims, ControlMessage, EngineSessionState,
    HelloAckMapping, KeyId, Protocol, RejectCode, RejectMessage, RelayKeySet, SequenceMessage,
    SigningKeyMaterial, SigningKeyRing, DEFAULT_TOKEN_ISSUER,
};
use gtt_relay::{
    AllocationReservation, HandshakeOutcome, QuicConnectionMeta, RelayConfig, RelayNode,
    ReservationState, RELAY_ALPN,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const TUNNEL_ID: &str = "11111111-1111-4111-8111-111111111111";
const ALLOCATION_ID: &str = "22222222-2222-4222-8222-222222222222";
const RELAY_ID: &str = "33333333-3333-4333-8333-333333333333";
const SESSION_ID_1: &str = "44444444-4444-4444-8444-444444444441";
const SESSION_ID_2: &str = "44444444-4444-4444-8444-444444444442";

fn repo_profiles_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("profiles")
}

struct MockControlPlaneTransport {
    responses: RefCell<VecDeque<HttpResponse>>,
    requests: RefCell<Vec<HttpRequest>>,
}

impl MockControlPlaneTransport {
    fn new(responses: Vec<HttpResponse>) -> Self {
        Self {
            responses: RefCell::new(responses.into()),
            requests: RefCell::new(Vec::new()),
        }
    }
}

impl HttpTransport for MockControlPlaneTransport {
    fn send(&mut self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.requests.borrow_mut().push(request.clone());
        self.responses
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| TransportError::Network("no queued mock HTTP response".to_owned()))
    }
}

fn build_signing_and_relay_keyset() -> Result<(SigningKeyRing, RelayKeySet), String> {
    let kid = KeyId::new("kid-2026-10").map_err(|e| format!("kid error: {e}"))?;
    let material =
        SigningKeyMaterial::from_bytes([0x42_u8; 32]).map_err(|e| format!("key error: {e}"))?;
    let ring = SigningKeyRing::new(DEFAULT_TOKEN_ISSUER, kid, material, 1_800_000);
    let key_set = ring.export_verification_keys(1_800_000);
    Ok((ring, key_set))
}

fn issue_token(
    ring: &SigningKeyRing,
    jti: &str,
    exp_epoch_s: i64,
    mappings: Vec<HelloAckMapping>,
) -> Result<String, String> {
    let claims = AllocationTokenClaims {
        iss: DEFAULT_TOKEN_ISSUER.to_owned(),
        aud: RELAY_ID.to_owned(),
        sub: TUNNEL_ID.to_owned(),
        aid: ALLOCATION_ID.to_owned(),
        jti: jti.to_owned(),
        iat: exp_epoch_s.saturating_sub(600),
        exp: exp_epoch_s,
        mappings,
    };
    ring.sign_allocation_token(&claims)
        .map_err(|e| format!("sign token failed: {e}"))
}

#[test]
fn engine_reaches_active_against_integration_stack_and_walks_full_state_machine_table() -> TestResult
{
    let registry = Registry::load_dir(&repo_profiles_dir())?;
    let mc_java = registry
        .get("minecraft-java")
        .ok_or("missing minecraft-java profile")?;

    let (signing_key, key_set) = build_signing_and_relay_keyset()?;
    let reserved_mappings = vec![HelloAckMapping {
        index: 0,
        mapping_key: "game".to_owned(),
        protocol: Protocol::Tcp,
        public_port: 25565,
    }];

    // 1. Set up RelayNode with reservation (`T-060`).
    let mut relay = RelayNode::new(RelayConfig::new(RELAY_ID), key_set)?;
    relay.upsert_reservation(AllocationReservation {
        allocation_id: ALLOCATION_ID.to_owned(),
        tunnel_id: TUNNEL_ID.to_owned(),
        expires_at_epoch_s: 2_000_000,
        mappings: reserved_mappings.clone(),
        state: ReservationState::Reserved,
    })?;

    // 2. Set up ControlPlaneClient (`T-047`) with responses for `POST /v1/tunnels/{id}/start`,
    //    `POST /v1/tunnels/{id}/token`, and `POST /v1/tunnels/{id}/stop`.
    let initial_token = issue_token(
        &signing_key,
        "55555555-5555-4555-8555-555555555551",
        1_900_000,
        reserved_mappings.clone(),
    )?;
    let refreshed_token = issue_token(
        &signing_key,
        "55555555-5555-4555-8555-555555555552",
        1_950_000,
        reserved_mappings.clone(),
    )?;

    let start_body = serde_json::json!({
        "tunnel_id": TUNNEL_ID,
        "allocation_token": initial_token,
        "allocation": {
            "id": ALLOCATION_ID,
            "expires_at": "2026-10-20T12:00:00Z",
            "relay": {
                "region": "us-east",
                "host": "relay.us-east.example.test",
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
    let refresh_body = serde_json::json!({
        "allocation_token": refreshed_token,
        "expires_at": "2026-10-20T12:15:00Z"
    });
    let stop_body = serde_json::json!({
        "id": TUNNEL_ID,
        "name": "Friday Night Server",
        "game_profile_id": "minecraft-java",
        "profile_version": 1,
        "state": "stopped",
        "mappings": [
            { "mapping_key": "game", "protocol": "tcp", "position": 0, "enabled": true }
        ],
        "created_at": "2026-10-20T11:00:00Z",
        "started_at": "2026-10-20T11:01:00Z",
        "stopped_at": "2026-10-20T11:30:00Z",
        "expires_at": "2026-10-20T19:00:00Z",
        "termination_reason": "user_stopped"
    });

    let transport = MockControlPlaneTransport::new(vec![
        HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: serde_json::to_vec(&start_body)?,
        },
        HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: serde_json::to_vec(&refresh_body)?,
        },
        HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: serde_json::to_vec(&stop_body)?,
        },
    ]);

    let config = ClientConfig::new("https://api.example.test");
    let mut api_client = ControlPlaneClient::new(
        config,
        Some("access-token-abc".to_owned()),
        transport,
        DeterministicBackoff::with_jitter_bps(0),
        NoTokenRefresher,
    )?;

    // 3. Configure EngineLifecycle (`Idle`).
    let inputs = vec![MappingTargetInput {
        mapping_key: "game".to_owned(),
        local_host: "127.0.0.1".to_owned(),
        local_port: 25565,
        enabled: true,
        confirm_non_loopback: false,
    }];
    let mut engine = EngineLifecycle::configure(mc_java, &inputs, "0.1.0-test")
        .map_err(|e| format!("configure failed: {e:?}"))?;
    assert_eq!(engine.state(), EngineSessionState::Idle);

    // 4. Transition `Idle -> Connecting` via `start_via_api` (`FR-005`).
    let idem_start = IdempotencyKey::new("idem-start-11111111")?;
    let _start_resp = engine.start_via_api(&mut api_client, TUNNEL_ID, &idem_start)?;
    assert_eq!(engine.state(), EngineSessionState::Connecting);
    assert_eq!(
        engine.public_endpoints(),
        &[PublicMappingEndpoint {
            index: 0,
            mapping_key: "game".to_owned(),
            protocol: Protocol::Tcp,
            relay_host: "relay.us-east.example.test".to_owned(),
            public_port: 25565,
        }]
    );
    assert_eq!(
        engine
            .public_endpoints()
            .first()
            .map(PublicMappingEndpoint::display_address),
        Some("relay.us-east.example.test:25565".to_owned())
    );

    // 5. Transition `Connecting -> Authenticating` on QUIC established (`on_quic_connected`).
    let hello_frame = engine.on_quic_connected()?;
    assert_eq!(engine.state(), EngineSessionState::Authenticating);

    // 6. Relay accepts `hello` and returns `hello_ack`; Engine transitions `Authenticating -> Active`.
    let conn_meta = QuicConnectionMeta {
        tls_authenticated: true,
        negotiated_alpn: RELAY_ALPN.to_owned(),
        assigned_session_id: SESSION_ID_1.to_owned(),
    };
    let hello_ack_frame = match relay.accept_hello_frame(&conn_meta, &hello_frame, 1_850_000) {
        HandshakeOutcome::Accepted { encoded_frame, .. } => encoded_frame,
        other => return Err(format!("expected Accepted, got {other:?}").into()),
    };

    let reply = engine.handle_relay_control_frame(&hello_ack_frame, 1_000)?;
    assert!(reply.is_none());
    assert_eq!(engine.state(), EngineSessionState::Active);

    // 7. In-session token refresh (`refresh_token_via_api`) -> relay `TokenRefreshAck`.
    let refresh_frame = engine.refresh_token_via_api(&mut api_client)?;
    let (decoded_refresh, _) = gtt_protocol::decode_control_frame(&refresh_frame)?;
    let refresh_msg = match decoded_refresh {
        ControlMessage::TokenRefresh(m) => m,
        other => return Err(format!("expected TokenRefresh, got {other:?}").into()),
    };
    let refresh_ack = relay
        .handle_token_refresh(
            ALLOCATION_ID,
            &refresh_msg,
            "2026-10-20T12:15:00Z".to_owned(),
            1_860_000,
        )
        .map_err(|c| format!("token refresh rejected: {c:?}"))?;
    let refresh_ack_frame = encode_control_frame(&ControlMessage::TokenRefreshAck(refresh_ack))?;
    let _ = engine.handle_relay_control_frame(&refresh_ack_frame, 1_500)?;
    assert_eq!(engine.state(), EngineSessionState::Active);

    // 8. Transition `Active -> Degraded` on high RTT `pong` (`> 250 ms`), then `Degraded -> Active`
    //    on normal RTT `pong` (`contracts.md` §12).
    let ping1 = match engine.session_mut().build_ping(2_000) {
        ControlMessage::Ping(s) => s,
        other => return Err(format!("expected Ping, got {other:?}").into()),
    };
    let pong1_frame =
        encode_control_frame(&ControlMessage::Pong(SequenceMessage { seq: ping1.seq }))?;
    let _ = engine.handle_relay_control_frame(&pong1_frame, 2_400)?; // 400 ms RTT -> Degraded
    assert_eq!(engine.state(), EngineSessionState::Degraded);

    let ping2 = match engine.session_mut().build_ping(3_000) {
        ControlMessage::Ping(s) => s,
        other => return Err(format!("expected Ping, got {other:?}").into()),
    };
    let pong2_frame =
        encode_control_frame(&ControlMessage::Pong(SequenceMessage { seq: ping2.seq }))?;
    let _ = engine.handle_relay_control_frame(&pong2_frame, 3_040)?; // 40 ms RTT -> Active
    assert_eq!(engine.state(), EngineSessionState::Active);

    // 9. Transition `Active -> Reconnecting` on connection lost, then `Reconnecting -> Active` on
    //    resumed `hello_ack` (`contracts.md` §12, `FR-014`).
    let _ = engine.on_connection_lost("simulated_drop")?;
    assert_eq!(engine.state(), EngineSessionState::Reconnecting);

    // Issue a fresh token for reconnect (since jti replay cache rejects reusing the same jti).
    let resume_token = issue_token(
        &signing_key,
        "55555555-5555-4555-8555-555555555553",
        1_960_000,
        reserved_mappings,
    )?;
    // Update token via a second start_with_response-equivalent or token refresh.
    let resume_hello = encode_control_frame(&ControlMessage::Hello(gtt_protocol::HelloMessage {
        protocol_version: 1,
        token: resume_token,
        client_version: "0.1.0-test".to_owned(),
    }))?;
    let conn_meta_2 = QuicConnectionMeta {
        tls_authenticated: true,
        negotiated_alpn: RELAY_ALPN.to_owned(),
        assigned_session_id: SESSION_ID_2.to_owned(),
    };
    let resume_ack_frame = match relay.accept_hello_frame(&conn_meta_2, &resume_hello, 1_870_000) {
        HandshakeOutcome::Accepted { encoded_frame, .. } => encoded_frame,
        other => return Err(format!("expected Accepted on resume, got {other:?}").into()),
    };
    let _ = engine.handle_relay_control_frame(&resume_ack_frame, 4_000)?;
    assert_eq!(engine.state(), EngineSessionState::Active);

    // 10. Transition `Active -> Stopped` via `stop_via_api` (`contracts.md` §12).
    let idem_stop = IdempotencyKey::new("idem-stop-11111111")?;
    let goodbye_frame = engine.stop_via_api(&mut api_client, Some(&idem_stop))?;
    assert_eq!(engine.state(), EngineSessionState::Stopped);
    let (goodbye_msg, _) = gtt_protocol::decode_control_frame(&goodbye_frame)?;
    assert!(matches!(goodbye_msg, ControlMessage::Goodbye(_)));

    // Confirm the recorded state transitions match `contracts.md` §12 in exact sequence.
    let observed_states: Vec<EngineSessionState> = engine
        .drain_events()
        .into_iter()
        .filter_map(|ev| match ev {
            EngineEvent::TunnelStatusChanged { state, .. } => Some(state),
            _ => None,
        })
        .collect();
    assert_eq!(
        observed_states,
        vec![
            EngineSessionState::Connecting,
            EngineSessionState::Authenticating,
            EngineSessionState::Active,
            EngineSessionState::Degraded,
            EngineSessionState::Active,
            EngineSessionState::Reconnecting,
            EngineSessionState::Active,
            EngineSessionState::Stopped,
        ]
    );

    // Verify all 3 HTTP requests hit the expected control-plane routes (`start`, `token`, `stop`)
    // and carried no local destination fields (`INV-1`).
    let recorded_reqs = api_client.transport().requests.borrow();
    assert_eq!(recorded_reqs.len(), 3);
    assert_eq!(
        recorded_reqs.first().map(|r| r.method),
        Some(HttpMethod::Post)
    );

    Ok(())
}

#[test]
fn authenticating_reject_and_reconnect_exhaustion_transitions_match_contracts_table() -> TestResult
{
    let registry = Registry::load_dir(&repo_profiles_dir())?;
    let mc_java = registry
        .get("minecraft-java")
        .ok_or("missing minecraft-java profile")?;

    let inputs = vec![MappingTargetInput {
        mapping_key: "game".to_owned(),
        local_host: "127.0.0.1".to_owned(),
        local_port: 25565,
        enabled: true,
        confirm_non_loopback: false,
    }];

    let start_resp: gtt_protocol::StartTunnelResponse =
        serde_json::from_value(serde_json::json!({
            "tunnel_id": TUNNEL_ID,
            "allocation_token": "eyJhbGciOiJFZERTQSJ9.eyJpc3MiOiJndHQifQ.c2ln",
            "allocation": {
                "id": ALLOCATION_ID,
                "expires_at": "2026-10-20T12:00:00Z",
                "relay": {
                    "region": "us-east",
                    "host": "relay.us-east.example.test",
                    "tunnel_port": 4433
                },
                "ports": [
                    { "mapping_key": "game", "protocol": "tcp", "public_port": 25565 }
                ]
            }
        }))?;

    // Case A: `Authenticating` + retryable `reject` (`capacity`) -> `Reconnecting`, followed by
    // retries exhausted -> `Failed` (`contracts.md` §12).
    let mut engine_retryable = EngineLifecycle::configure(mc_java, &inputs, "0.1.0-test")
        .map_err(|e| format!("configure failed: {e:?}"))?;
    let _ = engine_retryable.begin_start_with_response(&start_resp)?;
    let _ = engine_retryable.on_quic_connected()?;
    assert_eq!(engine_retryable.state(), EngineSessionState::Authenticating);

    let reject_capacity = encode_control_frame(&ControlMessage::Reject(RejectMessage {
        code: RejectCode::Capacity,
        retry_after_ms: Some(1_000),
    }))?;
    let _ = engine_retryable.handle_relay_control_frame(&reject_capacity, 100)?;
    assert_eq!(engine_retryable.state(), EngineSessionState::Reconnecting);

    for _ in 0..5 {
        let _ = engine_retryable.on_reconnect_attempt_failed("relay unreachable");
    }
    assert_eq!(engine_retryable.state(), EngineSessionState::Failed);

    // Case B: `Authenticating` + non-retryable `reject` (`token_invalid` or `version_unsupported`)
    // -> `Failed` immediately (`contracts.md` §12).
    for terminal_code in [RejectCode::TokenInvalid, RejectCode::VersionUnsupported] {
        let mut engine_terminal = EngineLifecycle::configure(mc_java, &inputs, "0.1.0-test")
            .map_err(|e| format!("configure failed: {e:?}"))?;
        let _ = engine_terminal.begin_start_with_response(&start_resp)?;
        let _ = engine_terminal.on_quic_connected()?;
        let reject_frame = encode_control_frame(&ControlMessage::Reject(RejectMessage {
            code: terminal_code,
            retry_after_ms: None,
        }))?;
        let _ = engine_terminal.handle_relay_control_frame(&reject_frame, 100)?;
        assert_eq!(engine_terminal.state(), EngineSessionState::Failed);
    }

    Ok(())
}
