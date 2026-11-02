//! Conformance and malformed-input tests for `gtt-protocol` (`T-031`, `TEST-001`, `NET-007`).
//!
//! ## No `expect` and no `panic!`
//!
//! The workspace denies the `panic`, `unwrap_used`, and `expect_used` lints on all targets
//! including tests, so every test returns [`TestResult`] and propagates errors with `?`.

use std::fs;
use std::path::{Path, PathBuf};

use gtt_protocol::{
    decode_control_frame, encode_control_frame, encode_udp_datagram, is_local_target_field,
    is_valid_rfc3339_utc, is_valid_uuid, validate_contract_payload, ApiErrorCode, ControlMessage,
    EngineSessionEvent, EngineSessionState, FlowSummaryMessage, FrameError, GoodbyeMessage,
    HelloAckMapping, HelloAckMessage, HelloMessage, Protocol, RejectCode, RejectMessage,
    SequenceMessage, SessionLimits, TcpOpenHeader, TcpOpenStatus, TokenRefreshAckMessage,
    TokenRefreshMessage, UdpDatagram, ValidationError, CONTROL_PREFIX_LEN,
    DEFAULT_MAX_DATAGRAM_BYTES, MAX_CONTROL_FRAME_BYTES, TCP_OPEN_FRAME_TYPE, TCP_OPEN_HEADER_LEN,
    UDP_HEADER_LEN,
};
use serde::Deserialize;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// All eleven JSON Schemas required by `contracts.md` and `scripts/audit_traceability.py`.
const EXPECTED_SCHEMAS: &[&str] = &[
    "contracts/error.schema.json",
    "contracts/error-response.schema.json",
    "contracts/tunnel-hello.schema.json",
    "contracts/tunnel-hello-ack.schema.json",
    "contracts/create-tunnel-request.schema.json",
    "contracts/start-tunnel-response.schema.json",
    "contracts/create-invitation-request.schema.json",
    "contracts/create-invitation-response.schema.json",
    "contracts/claim-invitation-request.schema.json",
    "contracts/claim-invitation-response.schema.json",
    "contracts/tunnel.schema.json",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

#[derive(Debug, Deserialize)]
struct ExampleSuite {
    version: u32,
    cases: Vec<ExampleCase>,
}

#[derive(Debug, Deserialize)]
struct ExampleCase {
    id: String,
    schema: String,
    valid: bool,
    description: String,
    payload: serde_json::Value,
}

#[test]
fn all_eleven_contract_schemas_exist_and_declare_draft_2020_12() -> TestResult {
    let root = repo_root();
    for &rel_path in EXPECTED_SCHEMAS {
        let full = root.join(rel_path);
        let text = fs::read_to_string(&full)?;
        let val: serde_json::Value = serde_json::from_str(&text)?;
        assert_eq!(
            val.get("$schema").and_then(serde_json::Value::as_str),
            Some("https://json-schema.org/draft/2020-12/schema"),
            "{rel_path} must declare JSON Schema draft 2020-12"
        );
        assert_eq!(
            val.get("additionalProperties")
                .and_then(serde_json::Value::as_bool),
            Some(false),
            "{rel_path} must set top-level additionalProperties: false"
        );
        assert_no_local_target_properties_in_schema(rel_path, &val);
    }
    Ok(())
}

fn assert_no_local_target_properties_in_schema(schema_name: &str, val: &serde_json::Value) {
    match val {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::Object(props)) = map.get("properties") {
                for prop_name in props.keys() {
                    assert!(
                        !is_local_target_field(prop_name),
                        "{schema_name} declares forbidden INV-1 property {prop_name:?}"
                    );
                }
            }
            for child in map.values() {
                assert_no_local_target_properties_in_schema(schema_name, child);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                assert_no_local_target_properties_in_schema(schema_name, item);
            }
        }
        _ => {}
    }
}

#[test]
fn all_example_payloads_in_contracts_examples_json_pass_or_fail_as_specified() -> TestResult {
    let root = repo_root();
    let examples_path = root.join("contracts/examples.json");
    let raw = fs::read_to_string(&examples_path)?;
    let suite: ExampleSuite = serde_json::from_str(&raw)?;
    assert_eq!(suite.version, 1);
    assert!(
        suite.cases.len() >= 25,
        "expected comprehensive valid and invalid test vectors"
    );

    // Every schema in EXPECTED_SCHEMAS must be exercised by at least one valid and one
    // invalid example in contracts/examples.json.
    for &schema in EXPECTED_SCHEMAS {
        let has_valid = suite.cases.iter().any(|c| c.schema == schema && c.valid);
        let has_invalid = suite.cases.iter().any(|c| c.schema == schema && !c.valid);
        assert!(
            has_valid,
            "schema {schema} has no valid vector in contracts/examples.json"
        );
        assert!(
            has_invalid,
            "schema {schema} has no invalid vector in contracts/examples.json"
        );
    }

    for case in &suite.cases {
        let result = validate_contract_payload(&case.schema, &case.payload);
        if case.valid {
            assert!(
                result.is_ok(),
                "example {:?} ({}) expected VALID for {}, but failed with: {:?}",
                case.id,
                case.description,
                case.schema,
                result.err()
            );
        } else {
            assert!(
                result.is_err(),
                "example {:?} ({}) expected INVALID for {}, but was accepted",
                case.id,
                case.description,
                case.schema
            );
        }
    }
    Ok(())
}

#[test]
fn control_stream_framing_roundtrips_all_nine_message_variants() -> TestResult {
    let messages = vec![
        ControlMessage::Hello(HelloMessage {
            protocol_version: 1,
            token: "tok_example".to_owned(),
            client_version: "0.1.0".to_owned(),
        }),
        ControlMessage::HelloAck(HelloAckMessage {
            session_id: "3f2b8c1e-5a47-4d0b-9c1e-0a6f1d2e7b44".to_owned(),
            heartbeat_interval_ms: 5000,
            idle_timeout_ms: 15000,
            limits: SessionLimits {
                max_tcp_flows: 64,
                max_udp_flows: 64,
            },
            mappings: vec![HelloAckMapping {
                index: 0,
                mapping_key: "game".to_owned(),
                protocol: Protocol::Tcp,
                public_port: 20001,
            }],
        }),
        ControlMessage::Reject(RejectMessage {
            code: RejectCode::Capacity,
            retry_after_ms: Some(2500),
        }),
        ControlMessage::Ping(SequenceMessage { seq: 1 }),
        ControlMessage::Pong(SequenceMessage { seq: 1 }),
        ControlMessage::TokenRefresh(TokenRefreshMessage {
            token: "tok_refreshed".to_owned(),
        }),
        ControlMessage::TokenRefreshAck(TokenRefreshAckMessage {
            expires_at: "2026-10-16T12:30:00Z".to_owned(),
        }),
        ControlMessage::FlowSummary(FlowSummaryMessage {
            tcp_flows: 2,
            udp_flows: 3,
            distinct_sources: 2,
        }),
        ControlMessage::Goodbye(GoodbyeMessage {
            reason: "host_stopped".to_owned(),
        }),
    ];

    for msg in messages {
        let mut framed = encode_control_frame(&msg)?;
        let expected_len = framed.len();
        // Append trailing bytes from the next frame to ensure decode_control_frame consumes
        // only the exact declared length.
        framed.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        let (decoded, consumed) = decode_control_frame(&framed)?;
        assert_eq!(decoded, msg);
        assert_eq!(consumed, expected_len);
    }
    Ok(())
}

#[test]
fn malformed_control_frames_return_typed_errors_without_panic() {
    // 1. Empty / truncated prefix (< 4 bytes)
    for len in 0..CONTROL_PREFIX_LEN {
        let buf = vec![0u8; len];
        assert_eq!(
            decode_control_frame(&buf),
            Err(FrameError::TruncatedControlPrefix { actual_len: len })
        );
    }

    // 2. Zero-length frame is rejected before reading payload
    assert_eq!(
        decode_control_frame(&[0, 0, 0, 0]),
        Err(FrameError::ControlFrameLengthOutOfBounds {
            declared_len: 0,
            max_len: MAX_CONTROL_FRAME_BYTES,
        })
    );

    // 3. Oversized length prefix (e.g. u32::MAX or MAX_CONTROL_FRAME_BYTES + 1) is rejected
    //    immediately without allocating or reading payload bytes.
    let over = u32::try_from(MAX_CONTROL_FRAME_BYTES + 1).unwrap_or(u32::MAX);
    assert_eq!(
        decode_control_frame(&over.to_be_bytes()),
        Err(FrameError::ControlFrameLengthOutOfBounds {
            declared_len: MAX_CONTROL_FRAME_BYTES + 1,
            max_len: MAX_CONTROL_FRAME_BYTES,
        })
    );
    assert_eq!(
        decode_control_frame(&u32::MAX.to_be_bytes()),
        Err(FrameError::ControlFrameLengthOutOfBounds {
            declared_len: u32::MAX as usize,
            max_len: MAX_CONTROL_FRAME_BYTES,
        })
    );

    // 4. Truncated payload (prefix says 20 bytes, only 5 present)
    let mut truncated = Vec::new();
    truncated.extend_from_slice(&20u32.to_be_bytes());
    truncated.extend_from_slice(b"{\"typ");
    assert_eq!(
        decode_control_frame(&truncated),
        Err(FrameError::TruncatedControlPayload {
            declared_len: 20,
            available_len: 5,
        })
    );

    // 5. Non-JSON / garbage bytes
    let garbage = b"\xff\xfe\xfdnot-json";
    let mut bad_frame = Vec::new();
    let garbage_len = u32::try_from(garbage.len()).unwrap_or(0);
    bad_frame.extend_from_slice(&garbage_len.to_be_bytes());
    bad_frame.extend_from_slice(garbage);
    assert_eq!(
        decode_control_frame(&bad_frame),
        Err(FrameError::MalformedControlJson)
    );

    // 6. Control frame smuggling a local target field (INV-1)
    let inv1_json = br#"{"type":"ping","seq":1,"local_port":25565}"#;
    let mut inv1_frame = Vec::new();
    let inv1_len = u32::try_from(inv1_json.len()).unwrap_or(0);
    inv1_frame.extend_from_slice(&inv1_len.to_be_bytes());
    inv1_frame.extend_from_slice(inv1_json);
    assert_eq!(
        decode_control_frame(&inv1_frame),
        Err(FrameError::InvalidControlPayload(
            ValidationError::LocalTargetField("local_port".to_owned())
        ))
    );
}

#[test]
fn tcp_open_header_and_status_framing_reject_malformed_inputs() {
    let header = TcpOpenHeader {
        mapping_index: 0x0207,
    };
    let bytes = header.encode();
    assert_eq!(bytes, [TCP_OPEN_FRAME_TYPE, 0x02, 0x07]);
    assert_eq!(bytes.len(), TCP_OPEN_HEADER_LEN);
    assert_eq!(TcpOpenHeader::decode(&bytes), Ok(header));

    // Truncated TCP open headers
    assert_eq!(
        TcpOpenHeader::decode(&[]),
        Err(FrameError::TruncatedTcpOpenHeader { actual_len: 0 })
    );
    assert_eq!(
        TcpOpenHeader::decode(&[TCP_OPEN_FRAME_TYPE, 0x00]),
        Err(FrameError::TruncatedTcpOpenHeader { actual_len: 2 })
    );

    // Unknown TCP frame type
    assert_eq!(
        TcpOpenHeader::decode(&[0x00, 0x00, 0x00]),
        Err(FrameError::UnknownTcpFrameType(0x00))
    );
    assert_eq!(
        TcpOpenHeader::decode(&[0xff, 0x00, 0x01]),
        Err(FrameError::UnknownTcpFrameType(0xff))
    );

    // TCP open status bytes
    assert_eq!(TcpOpenStatus::from_byte(0x00), Ok(TcpOpenStatus::Connected));
    assert_eq!(TcpOpenStatus::from_byte(0x01), Ok(TcpOpenStatus::Refused));
    assert_eq!(
        TcpOpenStatus::from_byte(0x02),
        Ok(TcpOpenStatus::LimitReached)
    );
    assert_eq!(TcpOpenStatus::Connected.to_byte(), 0x00);
    assert_eq!(TcpOpenStatus::Refused.to_byte(), 0x01);
    assert_eq!(TcpOpenStatus::LimitReached.to_byte(), 0x02);
    for invalid in [0x03u8, 0x04, 0x7f, 0xff] {
        assert_eq!(
            TcpOpenStatus::from_byte(invalid),
            Err(FrameError::UnknownTcpOpenStatus(invalid))
        );
    }
}

#[test]
fn udp_datagram_framing_enforces_header_and_mtu_bounds() -> TestResult {
    let payload = b"terraria-packet";
    let encoded = encode_udp_datagram(3, 0x0102_0304, payload, DEFAULT_MAX_DATAGRAM_BYTES)?;
    assert_eq!(encoded.len(), UDP_HEADER_LEN + payload.len());

    let decoded = UdpDatagram::decode(&encoded, DEFAULT_MAX_DATAGRAM_BYTES)?;
    assert_eq!(decoded.mapping_index, 3);
    assert_eq!(decoded.flow_id, 0x0102_0304);
    assert_eq!(decoded.payload, payload);

    // Empty payload (6-byte header only) is valid
    let empty = encode_udp_datagram(0, 1, &[], DEFAULT_MAX_DATAGRAM_BYTES)?;
    let decoded_empty = UdpDatagram::decode(&empty, DEFAULT_MAX_DATAGRAM_BYTES)?;
    assert!(decoded_empty.payload.is_empty());

    // Truncated headers (< 6 bytes) must fail without panicking
    for len in 0..UDP_HEADER_LEN {
        let short = vec![0u8; len];
        assert_eq!(
            UdpDatagram::decode(&short, DEFAULT_MAX_DATAGRAM_BYTES),
            Err(FrameError::TruncatedUdpHeader { actual_len: len })
        );
    }

    // Oversized datagram (> max_datagram_bytes) must return OversizedDatagram so the caller
    // drops and increments datagrams_dropped (contracts.md section 11).
    let max_payload_len = DEFAULT_MAX_DATAGRAM_BYTES - UDP_HEADER_LEN;
    let exact_fit = vec![0xabu8; max_payload_len];
    assert!(encode_udp_datagram(1, 42, &exact_fit, DEFAULT_MAX_DATAGRAM_BYTES).is_ok());

    let one_byte_over = vec![0xabu8; max_payload_len + 1];
    assert_eq!(
        encode_udp_datagram(1, 42, &one_byte_over, DEFAULT_MAX_DATAGRAM_BYTES),
        Err(FrameError::OversizedDatagram {
            datagram_len: DEFAULT_MAX_DATAGRAM_BYTES + 1,
            max_datagram_bytes: DEFAULT_MAX_DATAGRAM_BYTES,
        })
    );

    let raw_over = vec![0u8; DEFAULT_MAX_DATAGRAM_BYTES + 1];
    assert_eq!(
        UdpDatagram::decode(&raw_over, DEFAULT_MAX_DATAGRAM_BYTES),
        Err(FrameError::OversizedDatagram {
            datagram_len: DEFAULT_MAX_DATAGRAM_BYTES + 1,
            max_datagram_bytes: DEFAULT_MAX_DATAGRAM_BYTES,
        })
    );

    // Misconfigured limit smaller than header
    assert_eq!(
        UdpDatagram::decode(&[0u8; 6], 5),
        Err(FrameError::InvalidDatagramLimit {
            max_datagram_bytes: 5
        })
    );
    Ok(())
}

#[test]
fn engine_session_state_machine_matches_contracts_section_12() -> TestResult {
    let s = EngineSessionState::Idle;
    let s = s.transition(EngineSessionEvent::StartWithToken)?;
    assert_eq!(s, EngineSessionState::Connecting);

    let s = s.transition(EngineSessionEvent::QuicEstablished)?;
    assert_eq!(s, EngineSessionState::Authenticating);

    // Retryable rejection -> Reconnecting -> Active -> Degraded -> Active -> ConnectionLost -> Reconnecting -> Failed
    let reconnecting = s.transition(EngineSessionEvent::Rejected(RejectCode::Capacity))?;
    assert_eq!(reconnecting, EngineSessionState::Reconnecting);

    let active = reconnecting.transition(EngineSessionEvent::ReconnectSucceeded)?;
    assert_eq!(active, EngineSessionState::Active);

    let degraded = active.transition(EngineSessionEvent::HeartbeatLostOrHighLatency)?;
    assert_eq!(degraded, EngineSessionState::Degraded);

    let recovered = degraded.transition(EngineSessionEvent::Recovered)?;
    assert_eq!(recovered, EngineSessionState::Active);

    let lost = recovered.transition(EngineSessionEvent::ConnectionLost)?;
    assert_eq!(lost, EngineSessionState::Reconnecting);

    let failed = lost.transition(EngineSessionEvent::RetriesExhausted)?;
    assert_eq!(failed, EngineSessionState::Failed);
    assert!(failed.is_terminal());

    // Terminal Failed rejects further transitions
    assert!(failed
        .transition(EngineSessionEvent::ReconnectSucceeded)
        .is_err());

    // Non-retryable rejections (`token_invalid`, `version_unsupported`) go straight to Failed
    for fatal in [RejectCode::TokenInvalid, RejectCode::VersionUnsupported] {
        assert_eq!(
            EngineSessionState::Authenticating.transition(EngineSessionEvent::Rejected(fatal)),
            Ok(EngineSessionState::Failed)
        );
    }

    // Any non-terminal state transitions to Stopped on UserStop
    for state in [
        EngineSessionState::Idle,
        EngineSessionState::Connecting,
        EngineSessionState::Authenticating,
        EngineSessionState::Active,
        EngineSessionState::Degraded,
        EngineSessionState::Reconnecting,
        EngineSessionState::Stopped,
    ] {
        assert_eq!(
            state.transition(EngineSessionEvent::UserStop),
            Ok(EngineSessionState::Stopped)
        );
    }
    Ok(())
}

#[test]
fn uuid_and_rfc3339_format_validators_enforce_strict_rules() {
    assert!(is_valid_uuid("3f2b8c1e-5a47-4d0b-9c1e-0a6f1d2e7b44"));
    assert!(!is_valid_uuid("3f2b8c1e5a474d0b9c1e0a6f1d2e7b44"));
    assert!(!is_valid_uuid("3f2b8c1e-5a47-4d0b-9c1e-0a6f1d2e7b4z"));

    assert!(is_valid_rfc3339_utc("2026-10-16T12:00:00Z"));
    assert!(is_valid_rfc3339_utc("2024-02-29T23:59:59.123456789Z"));
    // 2026 is not a leap year
    assert!(!is_valid_rfc3339_utc("2026-02-29T12:00:00Z"));
    // Non-UTC offsets rejected (contracts.md section 1 requires UTC)
    assert!(!is_valid_rfc3339_utc("2026-10-16T12:00:00+02:00"));
    assert!(!is_valid_rfc3339_utc("2026-13-01T00:00:00Z"));

    assert_eq!(ApiErrorCode::IdempotencyMismatch.http_status(), 422);
    assert_eq!(ApiErrorCode::ClientUpgradeRequired.http_status(), 426);
    assert_eq!(ApiErrorCode::RelayUnavailable.http_status(), 503);

    let ok_token_resp = gtt_protocol::IssueTunnelTokenResponse {
        allocation_token: "hdr.claims.sig".to_owned(),
        expires_at: "2026-10-16T12:00:00Z".to_owned(),
    };
    assert!(ok_token_resp.validate().is_ok());

    let empty_token_resp = gtt_protocol::IssueTunnelTokenResponse {
        allocation_token: "   ".to_owned(),
        expires_at: "2026-10-16T12:00:00Z".to_owned(),
    };
    assert!(empty_token_resp.validate().is_err());

    let bad_ts_token_resp = gtt_protocol::IssueTunnelTokenResponse {
        allocation_token: "hdr.claims.sig".to_owned(),
        expires_at: "not-a-timestamp".to_owned(),
    };
    assert!(bad_ts_token_resp.validate().is_err());
}
