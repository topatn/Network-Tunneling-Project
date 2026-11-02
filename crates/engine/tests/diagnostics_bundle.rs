//! Automated security and leak-scan tests for `T-075` (`DiagnosticsBundle`, `FR-013`,
//! `PRIV-002`).

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::time::Duration;

use gtt_engine::{
    redact_diagnostic_text, scan_diagnostics_for_leaks, EngineLifecycle, MappingTargetInput,
    MAX_DIAGNOSTIC_LINE_CHARS, MAX_DIAGNOSTIC_LOG_LINES, REDACTED_INVITATION_MARKER,
    REDACTED_IP_MARKER, REDACTED_PAYLOAD_MARKER, REDACTED_SECRET_MARKER, REDACTED_TOKEN_MARKER,
};
use gtt_profiles::Registry;
use gtt_protocol::{
    encode_control_frame, ControlMessage, EngineSessionState, FlowSummaryMessage, HelloAckMapping,
    HelloAckMessage, Protocol, SessionLimits, StartTunnelResponse,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn repo_profiles_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("profiles")
}

#[test]
fn diagnostics_bundle_redacts_tokens_secrets_guest_ips_and_payload_bytes() -> TestResult {
    let registry = Registry::load_dir(&repo_profiles_dir())?;
    let mc_java = registry
        .get("minecraft-java")
        .ok_or("missing minecraft-java profile")?;

    let local_tcp = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
    let local_port = local_tcp.local_addr()?.port();

    let inputs = vec![MappingTargetInput {
        mapping_key: "game".to_owned(),
        local_host: "127.0.0.1".to_owned(),
        local_port,
        enabled: true,
        confirm_non_loopback: false,
    }];

    let mut engine = EngineLifecycle::configure(mc_java, &inputs, "0.1.0-test")
        .map_err(|e| format!("configure failed: {e:?}"))?;

    let planted_jws = "eyJhbGciOiJIUzI1NiIsImtpZCI6ImtpZC0xIn0.eyJzdWIiOiJ0dW4tMSJ9.c2lnbmF0dXJl";
    let start_resp: StartTunnelResponse = serde_json::from_value(serde_json::json!({
        "tunnel_id": "11111111-1111-4111-8111-111111111111",
        "allocation_token": planted_jws,
        "allocation": {
            "id": "22222222-2222-4222-8222-222222222222",
            "expires_at": "2026-10-27T20:00:00Z",
            "relay": {
                "region": "ap-southeast",
                "host": "relay.ap-southeast.example.test",
                "tunnel_port": 4433
            },
            "ports": [
                { "mapping_key": "game", "protocol": "tcp", "public_port": 25565 }
            ]
        }
    }))?;

    let _ = engine.begin_start_with_response(&start_resp)?;
    let _ = engine.on_quic_connected()?;

    let hello_ack = encode_control_frame(&ControlMessage::HelloAck(HelloAckMessage {
        session_id: "3f2b8c1e-5a47-4d0b-9c1e-0a6f1d2e7b44".to_owned(),
        heartbeat_interval_ms: 5_000,
        idle_timeout_ms: 15_000,
        limits: SessionLimits {
            max_tcp_flows: 32,
            max_udp_flows: 32,
        },
        mappings: vec![HelloAckMapping {
            index: 0,
            mapping_key: "game".to_owned(),
            protocol: Protocol::Tcp,
            public_port: 25565,
        }],
    }))?;
    let _ = engine.handle_relay_control_frame(&hello_ack, 100)?;
    assert_eq!(engine.state(), EngineSessionState::Active);

    // Emit flow summary and probe local target so all status indicators are populated.
    let summary_frame = encode_control_frame(&ControlMessage::FlowSummary(FlowSummaryMessage {
        tcp_flows: 2,
        udp_flows: 0,
        distinct_sources: 2,
    }))?;
    let _ = engine.handle_relay_control_frame(&summary_frame, 200)?;
    let _ = engine.probe_local(Some("game"), Duration::from_millis(250));

    // Plant every forbidden category into raw log lines:
    // 1. Bearer access token & refresh token
    // 2. Invitation secret (`gtt_inv_...`)
    // 3. 3-part JWS allocation token (`eyJ...`)
    // 4. RCON and MSMP server secrets (`rcon.password=...`, `management-server-secret=...`)
    // 5. Raw guest IPv4, IPv6, and Tailscale CGNAT addresses
    // 6. Hex payload dump (`>= 48` hex chars) and `payload_bytes=...`
    let planted_inv_secret = "gtt_inv_9z8y7x6w5v4u3t2s1r0qAbCdEfGhIjKl";
    let planted_rcon_secret = "RconTopSecretPassword999";
    let planted_msmp_secret = "MsmpGenerated43CharSecretValueAbcdefghijklm";
    let planted_guest_ipv4 = "198.51.100.77";
    let planted_guest_ipv4_port = "203.0.113.42:54321";
    let planted_guest_ipv6 = "[2001:db8::dead:beef]:25565";
    let planted_payload_hex = "deadbeefcafebabe00112233445566778899aabbccddeeff0011223344556677";

    let raw_logs = [
        format!("INFO http request Authorization: Bearer {planted_jws}"),
        format!("INFO share created secret={planted_inv_secret} for tunnel"),
        format!("WARN server config rcon.password={planted_rcon_secret} management-server-secret={planted_msmp_secret}"),
        format!("INFO guest flow opened from {planted_guest_ipv4} and peer={planted_guest_ipv4_port} ipv6={planted_guest_ipv6}"),
        format!("DEBUG udp datagram dropped payload_bytes={planted_payload_hex} raw={planted_payload_hex}"),
    ];
    let raw_log_refs: Vec<&str> = raw_logs.iter().map(String::as_str).collect();

    let bundle = engine.export_diagnostics_bundle(1_761_559_200_000, &raw_log_refs);
    assert!(bundle.redacted_logs.len() <= MAX_DIAGNOSTIC_LOG_LINES);
    let bundle_json = bundle.to_json_pretty()?;

    // Automated leak scan (`T-075` verification): must find 0 leaks.
    let leaks = scan_diagnostics_for_leaks(&bundle_json);
    assert!(
        leaks.is_empty(),
        "automated leak scan found sensitive data in diagnostics bundle: {leaks:?}\nbundle:\n{bundle_json}"
    );

    // Explicitly assert none of the planted secrets, raw IPs, or payload bytes appear anywhere.
    for forbidden_literal in [
        planted_jws,
        planted_inv_secret,
        planted_rcon_secret,
        planted_msmp_secret,
        planted_guest_ipv4,
        "203.0.113.42",
        "2001:db8::dead:beef",
        "127.0.0.1",
        planted_payload_hex,
    ] {
        assert!(
            !bundle_json.contains(forbidden_literal),
            "exported diagnostics bundle leaked `{forbidden_literal}`:\n{bundle_json}"
        );
    }

    // Confirm redaction markers are present where expected.
    assert!(bundle_json.contains(REDACTED_TOKEN_MARKER));
    assert!(bundle_json.contains(REDACTED_INVITATION_MARKER));
    assert!(bundle_json.contains(REDACTED_SECRET_MARKER));
    assert!(bundle_json.contains(REDACTED_IP_MARKER));
    assert!(bundle_json.contains(REDACTED_PAYLOAD_MARKER));

    Ok(())
}

#[test]
fn leak_scanner_catches_planted_tokens_ips_and_payloads_and_bundle_enforces_bounds() {
    // Negative check: verify `scan_diagnostics_for_leaks` catches each leak category if unredacted.
    let unredacted_sample = r#"{
        "log_1": "gtt_inv_AbCdEfGhIjKlMnOpQrStUvWxYz",
        "log_2": "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2ln",
        "log_3": "guest 198.51.100.99 connected",
        "log_4": "rcon.password=PlaintextSecret",
        "log_5": "deadbeefcafebabe00112233445566778899aabbccddeeff0011223344556677"
    }"#;
    let findings = scan_diagnostics_for_leaks(unredacted_sample);
    assert!(
        findings.len() >= 5,
        "expected leak scanner to flag all 5 planted leaks, got: {findings:?}"
    );

    // Verify line length bound (`MAX_DIAGNOSTIC_LINE_CHARS`).
    let overlong = "x".repeat(MAX_DIAGNOSTIC_LINE_CHARS + 200);
    let redacted_overlong = redact_diagnostic_text(&overlong);
    assert!(redacted_overlong.ends_with("...[truncated]"));
    assert!(redacted_overlong.chars().count() <= MAX_DIAGNOSTIC_LINE_CHARS + 15);
}
