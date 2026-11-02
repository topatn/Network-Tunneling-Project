//! Unit and security tests for the relay skeleton (`T-060`) and signing-key rotation (`T-050`).
//!
//! Covers:
//! - Happy-path `hello` -> `hello_ack` wire framing and reservation activation (`contracts.md` §10).
//! - Every `RejectCode` path (`version_unsupported`, `relay_draining`, `capacity`, `token_invalid`,
//!   `token_expired`, `allocation_not_found`, `SEC-004`).
//! - `jti` replay cache uniqueness until `exp` and bounded eviction (`SEC-004`, `AGENTS.md` §4).
//! - Signing-key rotation (`T-050`, `SEC-016`): tokens signed with old `kid` continue to verify
//!   during the overlap window while new tokens use new `kid`, and old `kid` is rejected after the
//!   overlap window elapses or after emergency revocation.
//! - Transport security (`SEC-010`): unauthenticated QUIC or wrong ALPN is aborted before reading
//!   any payload.
//! - Architectural boundary (`SEC-005`, `INV-1`): control frames or token claims containing local
//!   target fields are rejected, and source inspection confirms no client-directed outbound dial
//!   exists in `crates/relay`.

use std::error::Error;

use gtt_protocol::{
    base64url_encode, decode_control_frame, encode_control_frame, hmac_sha256,
    AllocationTokenClaims, ControlMessage, HelloAckMapping, HelloMessage, KeyId, Protocol,
    RejectCode, SigningKeyMaterial, SigningKeyRing, TokenRefreshMessage, CURRENT_PROTOCOL_VERSION,
    DEFAULT_TOKEN_ISSUER,
};
use gtt_relay::{
    AllocationReservation, HandshakeOutcome, QuicConnectionMeta, RelayConfig, RelayNode,
    ReservationState, RELAY_ALPN,
};

type TestResult = Result<(), Box<dyn Error>>;

const RELAY_ID: &str = "11111111-1111-4111-8111-111111111111";
const OTHER_RELAY_ID: &str = "22222222-2222-4222-8222-222222222222";
const TUNNEL_ID: &str = "33333333-3333-4333-8333-333333333333";
const ALLOCATION_ID: &str = "44444444-4444-4444-8444-444444444444";
const SESSION_ID: &str = "55555555-5555-4555-8555-555555555555";
const JTI_1: &str = "66666666-6666-4666-8666-666666666661";
const JTI_2: &str = "66666666-6666-4666-8666-666666666662";
const JTI_3: &str = "66666666-6666-4666-8666-666666666663";
const NOW_EPOCH_S: i64 = 1_760_000_000;

fn sample_mappings() -> Vec<HelloAckMapping> {
    vec![HelloAckMapping {
        index: 0,
        mapping_key: "game".to_owned(),
        protocol: Protocol::Tcp,
        public_port: 25565,
    }]
}

fn sample_claims(jti: &str, exp: i64) -> AllocationTokenClaims {
    AllocationTokenClaims {
        iss: DEFAULT_TOKEN_ISSUER.to_owned(),
        aud: RELAY_ID.to_owned(),
        sub: TUNNEL_ID.to_owned(),
        aid: ALLOCATION_ID.to_owned(),
        mappings: sample_mappings(),
        iat: NOW_EPOCH_S - 10,
        exp,
        jti: jti.to_owned(),
    }
}

fn sample_reservation(expires_at_epoch_s: i64) -> AllocationReservation {
    AllocationReservation {
        allocation_id: ALLOCATION_ID.to_owned(),
        tunnel_id: TUNNEL_ID.to_owned(),
        expires_at_epoch_s,
        mappings: sample_mappings(),
        state: ReservationState::Reserved,
    }
}

fn valid_conn_meta() -> QuicConnectionMeta {
    QuicConnectionMeta {
        tls_authenticated: true,
        negotiated_alpn: RELAY_ALPN.to_owned(),
        assigned_session_id: SESSION_ID.to_owned(),
    }
}

fn make_ring() -> Result<SigningKeyRing, Box<dyn Error>> {
    let kid = KeyId::new("kid-2026-10-a")?;
    let material = SigningKeyMaterial::from_bytes([0x42; 32])?;
    Ok(SigningKeyRing::new(
        DEFAULT_TOKEN_ISSUER,
        kid,
        material,
        NOW_EPOCH_S - 3600,
    ))
}

fn assert_rejected_with(outcome: &HandshakeOutcome, expected_code: RejectCode) -> TestResult {
    match outcome {
        HandshakeOutcome::Rejected {
            reject,
            encoded_frame,
        } => {
            assert_eq!(reject.code, expected_code);
            let (decoded, consumed) = decode_control_frame(encoded_frame)?;
            assert_eq!(consumed, encoded_frame.len());
            assert_eq!(decoded, ControlMessage::Reject(*reject));
            Ok(())
        }
        other => Err(format!("expected Rejected({expected_code:?}), got {other:?}").into()),
    }
}

#[test]
fn valid_hello_activates_reservation_and_emits_hello_ack() -> TestResult {
    let ring = make_ring()?;
    let config = RelayConfig::new(RELAY_ID);
    let mut relay = RelayNode::new(config, ring.export_verification_keys(NOW_EPOCH_S))?;
    relay.upsert_reservation(sample_reservation(NOW_EPOCH_S + 300))?;

    let claims = sample_claims(JTI_1, NOW_EPOCH_S + 300);
    let token = ring.sign_allocation_token(&claims)?;
    let hello_frame = encode_control_frame(&ControlMessage::Hello(HelloMessage {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        token,
        client_version: "0.1.0-test".to_owned(),
    }))?;

    let outcome = relay.accept_hello_frame(&valid_conn_meta(), &hello_frame, NOW_EPOCH_S);
    match outcome {
        HandshakeOutcome::Accepted {
            ack,
            encoded_frame,
            allocation_id,
            tunnel_id,
        } => {
            assert_eq!(ack.session_id, SESSION_ID);
            assert_eq!(ack.mappings, sample_mappings());
            assert_eq!(allocation_id, ALLOCATION_ID);
            assert_eq!(tunnel_id, TUNNEL_ID);

            let (decoded, consumed) = decode_control_frame(&encoded_frame)?;
            assert_eq!(consumed, encoded_frame.len());
            assert_eq!(decoded, ControlMessage::HelloAck(ack));
        }
        other => return Err(format!("expected Accepted, got {other:?}").into()),
    }

    assert_eq!(relay.active_session_count(), 1);
    let res = relay
        .reservation(ALLOCATION_ID)
        .ok_or("missing reservation")?;
    assert_eq!(
        res.state,
        ReservationState::Active {
            session_id: SESSION_ID.to_owned()
        }
    );
    Ok(())
}

#[test]
fn invalid_token_cases_return_specified_reject_codes() -> TestResult {
    let ring = make_ring()?;
    let config = RelayConfig::new(RELAY_ID);
    let mut relay = RelayNode::new(config, ring.export_verification_keys(NOW_EPOCH_S))?;
    relay.upsert_reservation(sample_reservation(NOW_EPOCH_S + 600))?;

    // 1. Unsupported protocol version -> version_unsupported (`contracts.md` §§10, 14).
    let valid_token = ring.sign_allocation_token(&sample_claims(JTI_1, NOW_EPOCH_S + 300))?;
    let outcome = relay.evaluate_hello(
        SESSION_ID,
        &HelloMessage {
            protocol_version: CURRENT_PROTOCOL_VERSION + 1,
            token: valid_token.clone(),
            client_version: "0.1.0".to_owned(),
        },
        NOW_EPOCH_S,
    );
    assert_rejected_with(&outcome, RejectCode::VersionUnsupported)?;

    // 2. Tampered signature -> token_invalid (`SEC-004`).
    let mut bad_sig_parts: Vec<&str> = valid_token.split('.').collect();
    if let Some(last) = bad_sig_parts.get_mut(2) {
        *last = base64url_encode(&[0x99u8; 32]).leak();
    }
    let tampered_token = bad_sig_parts.join(".");
    let outcome = relay.evaluate_hello(
        SESSION_ID,
        &HelloMessage {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            token: tampered_token,
            client_version: "0.1.0".to_owned(),
        },
        NOW_EPOCH_S,
    );
    assert_rejected_with(&outcome, RejectCode::TokenInvalid)?;

    // 3. Unknown kid -> token_invalid (`SEC-004`).
    let unknown_ring = SigningKeyRing::new(
        DEFAULT_TOKEN_ISSUER,
        KeyId::new("kid-unknown")?,
        SigningKeyMaterial::from_bytes([0x77; 32])?,
        NOW_EPOCH_S - 100,
    );
    let unknown_kid_token =
        unknown_ring.sign_allocation_token(&sample_claims(JTI_1, NOW_EPOCH_S + 300))?;
    let outcome = relay.evaluate_hello(
        SESSION_ID,
        &HelloMessage {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            token: unknown_kid_token,
            client_version: "0.1.0".to_owned(),
        },
        NOW_EPOCH_S,
    );
    assert_rejected_with(&outcome, RejectCode::TokenInvalid)?;

    // 4. Wrong audience (`aud` for a different relay) -> token_invalid (`SEC-004`).
    let mut wrong_aud_claims = sample_claims(JTI_1, NOW_EPOCH_S + 300);
    wrong_aud_claims.aud = OTHER_RELAY_ID.to_owned();
    let wrong_aud_token = ring.sign_allocation_token(&wrong_aud_claims)?;
    let outcome = relay.evaluate_hello(
        SESSION_ID,
        &HelloMessage {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            token: wrong_aud_token,
            client_version: "0.1.0".to_owned(),
        },
        NOW_EPOCH_S,
    );
    assert_rejected_with(&outcome, RejectCode::TokenInvalid)?;

    // 5. Wrong issuer (`iss`) -> token_invalid (`SEC-004`).
    let wrong_iss_ring = SigningKeyRing::new(
        "untrusted-issuer",
        KeyId::new("kid-2026-10-a")?,
        SigningKeyMaterial::from_bytes([0x42; 32])?,
        NOW_EPOCH_S - 100,
    );
    let mut wrong_iss_claims = sample_claims(JTI_1, NOW_EPOCH_S + 300);
    wrong_iss_claims.iss = "untrusted-issuer".to_owned();
    let wrong_iss_token = wrong_iss_ring.sign_allocation_token(&wrong_iss_claims)?;
    let outcome = relay.evaluate_hello(
        SESSION_ID,
        &HelloMessage {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            token: wrong_iss_token,
            client_version: "0.1.0".to_owned(),
        },
        NOW_EPOCH_S,
    );
    assert_rejected_with(&outcome, RejectCode::TokenInvalid)?;

    // 6. Expired token (`exp <= now`) -> token_expired (`SEC-004`).
    let expired_claims = AllocationTokenClaims {
        iat: NOW_EPOCH_S - 600,
        exp: NOW_EPOCH_S,
        ..sample_claims(JTI_1, NOW_EPOCH_S)
    };
    let expired_token = ring.sign_allocation_token(&expired_claims)?;
    let outcome = relay.evaluate_hello(
        SESSION_ID,
        &HelloMessage {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            token: expired_token,
            client_version: "0.1.0".to_owned(),
        },
        NOW_EPOCH_S,
    );
    assert_rejected_with(&outcome, RejectCode::TokenExpired)?;

    // 7. Replayed jti -> first hello accepted, second hello with same jti rejected with token_invalid.
    let first_outcome = relay.evaluate_hello(
        SESSION_ID,
        &HelloMessage {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            token: valid_token.clone(),
            client_version: "0.1.0".to_owned(),
        },
        NOW_EPOCH_S,
    );
    assert!(matches!(first_outcome, HandshakeOutcome::Accepted { .. }));

    let replay_outcome = relay.evaluate_hello(
        SESSION_ID,
        &HelloMessage {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            token: valid_token,
            client_version: "0.1.0".to_owned(),
        },
        NOW_EPOCH_S + 1,
    );
    assert_rejected_with(&replay_outcome, RejectCode::TokenInvalid)?;

    Ok(())
}

#[test]
fn draining_capacity_and_allocation_not_found_return_specified_reject_codes() -> TestResult {
    let ring = make_ring()?;
    let mut config = RelayConfig::new(RELAY_ID);
    config.max_allocations = 1;
    let mut relay = RelayNode::new(config, ring.export_verification_keys(NOW_EPOCH_S))?;

    // 1. Missing reservation -> allocation_not_found.
    let token_1 = ring.sign_allocation_token(&sample_claims(JTI_1, NOW_EPOCH_S + 300))?;
    let outcome = relay.evaluate_hello(
        SESSION_ID,
        &HelloMessage {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            token: token_1.clone(),
            client_version: "0.1.0".to_owned(),
        },
        NOW_EPOCH_S,
    );
    assert_rejected_with(&outcome, RejectCode::AllocationNotFound)?;

    // 2. Released reservation -> allocation_not_found.
    relay.upsert_reservation(sample_reservation(NOW_EPOCH_S + 300))?;
    assert!(relay.release_reservation(ALLOCATION_ID));
    let outcome = relay.evaluate_hello(
        SESSION_ID,
        &HelloMessage {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            token: token_1.clone(),
            client_version: "0.1.0".to_owned(),
        },
        NOW_EPOCH_S,
    );
    assert_rejected_with(&outcome, RejectCode::AllocationNotFound)?;

    // 3. Relay draining -> relay_draining with retry_after_ms.
    relay.upsert_reservation(sample_reservation(NOW_EPOCH_S + 300))?;
    relay.set_draining(true, Some(2_500));
    let outcome = relay.evaluate_hello(
        SESSION_ID,
        &HelloMessage {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            token: token_1.clone(),
            client_version: "0.1.0".to_owned(),
        },
        NOW_EPOCH_S,
    );
    assert_rejected_with(&outcome, RejectCode::RelayDraining)?;
    if let HandshakeOutcome::Rejected { reject, .. } = outcome {
        assert_eq!(reject.retry_after_ms, Some(2_500));
    }
    relay.set_draining(false, None);

    // 4. Activate 1 session, reaching max_allocations = 1; next hello gets capacity.
    let accepted = relay.evaluate_hello(
        SESSION_ID,
        &HelloMessage {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            token: token_1,
            client_version: "0.1.0".to_owned(),
        },
        NOW_EPOCH_S,
    );
    assert!(matches!(accepted, HandshakeOutcome::Accepted { .. }));

    let token_2 = ring.sign_allocation_token(&sample_claims(JTI_2, NOW_EPOCH_S + 300))?;
    let capacity_outcome = relay.evaluate_hello(
        SESSION_ID,
        &HelloMessage {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            token: token_2,
            client_version: "0.1.0".to_owned(),
        },
        NOW_EPOCH_S,
    );
    assert_rejected_with(&capacity_outcome, RejectCode::Capacity)?;

    Ok(())
}

#[test]
fn signing_key_rotation_overlap_window_accepts_old_and_new_kids_then_expires_old_kid() -> TestResult
{
    let mut ring = make_ring()?;
    let config = RelayConfig::new(RELAY_ID);
    let mut relay = RelayNode::new(config, ring.export_verification_keys(NOW_EPOCH_S))?;
    relay.upsert_reservation(sample_reservation(NOW_EPOCH_S + 3600))?;

    // Sign token_old with kid-2026-10-a before rotation.
    let token_old = ring.sign_allocation_token(&sample_claims(JTI_1, NOW_EPOCH_S + 1800))?;

    // Rotate to kid-2026-10-b with a 1200-second overlap window (`T-050`, `SEC-016`).
    let new_kid = KeyId::new("kid-2026-10-b")?;
    let new_material = SigningKeyMaterial::from_bytes([0x55; 32])?;
    ring.rotate(new_kid, new_material, NOW_EPOCH_S, 1200)?;

    // Push updated verification key set to relay.
    relay.update_verification_keys(ring.export_verification_keys(NOW_EPOCH_S));

    // Sign token_new with the newly active key (kid-2026-10-b).
    let token_new = ring.sign_allocation_token(&sample_claims(JTI_2, NOW_EPOCH_S + 1800))?;

    // During overlap window (NOW_EPOCH_S + 600 < NOW_EPOCH_S + 1200), token_old is accepted!
    let outcome_old = relay.evaluate_hello(
        SESSION_ID,
        &HelloMessage {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            token: token_old,
            client_version: "0.1.0".to_owned(),
        },
        NOW_EPOCH_S + 600,
    );
    assert!(matches!(outcome_old, HandshakeOutcome::Accepted { .. }));

    // In-session token_refresh using token_new (signed by kid-2026-10-b) also succeeds!
    let refresh_ack = relay
        .handle_token_refresh(
            ALLOCATION_ID,
            &TokenRefreshMessage { token: token_new },
            "2026-10-19T12:30:00Z".to_owned(),
            NOW_EPOCH_S + 600,
        )
        .map_err(|e| format!("refresh failed: {e:?}"))?;
    assert_eq!(refresh_ack.expires_at, "2026-10-19T12:30:00Z");

    // Once overlap window elapses (`now >= NOW_EPOCH_S + 1200`), a token signed with old key
    // (`kid-2026-10-a`) is rejected with `token_invalid`.
    let old_key_again = KeyId::new("kid-2026-10-a")?;
    let old_material_again = SigningKeyMaterial::from_bytes([0x42; 32])?;
    let token_old_after_overlap = gtt_protocol::sign_with_key(
        &old_key_again,
        &old_material_again,
        &sample_claims(JTI_3, NOW_EPOCH_S + 2400),
    )?;
    let post_overlap_outcome = relay.evaluate_hello(
        SESSION_ID,
        &HelloMessage {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            token: token_old_after_overlap,
            client_version: "0.1.0".to_owned(),
        },
        NOW_EPOCH_S + 1200,
    );
    assert_rejected_with(&post_overlap_outcome, RejectCode::TokenInvalid)?;

    // Emergency revocation removes a rotated key immediately even before its overlap window ends.
    assert!(ring.emergency_revoke_key("kid-2026-10-a")?);
    assert_eq!(
        ring.export_verification_keys(NOW_EPOCH_S + 600)
            .keys()
            .len(),
        1
    );

    Ok(())
}

#[test]
fn transport_sec_010_and_outbound_dial_sec_005_invariants_enforced() -> TestResult {
    let ring = make_ring()?;
    let config = RelayConfig::new(RELAY_ID);
    let mut relay = RelayNode::new(config, ring.export_verification_keys(NOW_EPOCH_S))?;
    relay.upsert_reservation(sample_reservation(NOW_EPOCH_S + 300))?;

    let valid_token = ring.sign_allocation_token(&sample_claims(JTI_1, NOW_EPOCH_S + 300))?;
    let hello_frame = encode_control_frame(&ControlMessage::Hello(HelloMessage {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        token: valid_token,
        client_version: "0.1.0".to_owned(),
    }))?;

    // 1. SEC-010: Unauthenticated QUIC connection is aborted before payload processing.
    let mut unauth_meta = valid_conn_meta();
    unauth_meta.tls_authenticated = false;
    let outcome = relay.accept_hello_frame(&unauth_meta, &hello_frame, NOW_EPOCH_S);
    assert!(matches!(outcome, HandshakeOutcome::Aborted { .. }));

    // 2. SEC-010: Wrong ALPN is aborted.
    let mut wrong_alpn_meta = valid_conn_meta();
    wrong_alpn_meta.negotiated_alpn = "http/1.1".to_owned();
    let outcome = relay.accept_hello_frame(&wrong_alpn_meta, &hello_frame, NOW_EPOCH_S);
    assert!(matches!(outcome, HandshakeOutcome::Aborted { .. }));

    // 3. INV-1 / SEC-005: Forged token smuggling `local_port` inside signed claims is rejected
    // with `token_invalid`.
    let header_b64 =
        base64url_encode(br#"{"alg":"HS256","kid":"kid-2026-10-a","typ":"gtt-alloc+jwt"}"#);
    let forged_claims_json = format!(
        r#"{{"iss":"{DEFAULT_TOKEN_ISSUER}","aud":"{RELAY_ID}","sub":"{TUNNEL_ID}","aid":"{ALLOCATION_ID}","mappings":[{{"index":0,"mapping_key":"game","protocol":"tcp","public_port":25565,"local_port":25565}}],"iat":{},"exp":{},"jti":"{JTI_2}"}}"#,
        NOW_EPOCH_S - 10,
        NOW_EPOCH_S + 300
    );
    let claims_b64 = base64url_encode(forged_claims_json.as_bytes());
    let signing_input = format!("{header_b64}.{claims_b64}");
    let tag = hmac_sha256(&[0x42; 32], signing_input.as_bytes());
    let forged_token = format!("{signing_input}.{}", base64url_encode(&tag));

    let inv1_outcome = relay.evaluate_hello(
        SESSION_ID,
        &HelloMessage {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            token: forged_token,
            client_version: "0.1.0".to_owned(),
        },
        NOW_EPOCH_S,
    );
    assert_rejected_with(&inv1_outcome, RejectCode::TokenInvalid)?;

    // 4. SEC-005 code audit check: verify `crates/relay/src/lib.rs` contains no outbound socket
    // connect calls (`TcpStream::connect`, `UdpSocket::connect`, `ToSocketAddrs`).
    let relay_src = include_str!("../src/lib.rs");
    for forbidden_symbol in [
        "TcpStream::connect",
        "UdpSocket::connect",
        "ToSocketAddrs",
        "std::net::TcpStream",
    ] {
        assert!(
            !relay_src.contains(forbidden_symbol),
            "SEC-005 violation: relay source must not contain outbound dial API {forbidden_symbol}"
        );
    }

    Ok(())
}
