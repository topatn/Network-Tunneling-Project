//! Network tests with simulated outage for heartbeats, idle timeouts, and session resumption
//! (`T-063`, `NET-004`, `NET-005`, `FR-014`).

use std::error::Error;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use gtt_engine::{EngineTunnelSession, LocalMappingConfig};
use gtt_protocol::{
    encode_udp_datagram, ControlMessage, EngineSessionState, HelloAckMapping, HelloAckMessage,
    Protocol, SequenceMessage, SessionLimits, TcpOpenHeader, UdpDatagram,
    DEFAULT_MAX_DATAGRAM_BYTES,
};

type TestResult = Result<(), Box<dyn Error>>;

const SESSION_ID_1: &str = "11111111-1111-4111-8111-111111111111";
const SESSION_ID_2: &str = "22222222-2222-4222-8222-222222222222";

fn sample_ack(session_id: &str) -> HelloAckMessage {
    HelloAckMessage {
        session_id: session_id.to_owned(),
        heartbeat_interval_ms: 5_000,
        idle_timeout_ms: 15_000,
        limits: SessionLimits {
            max_tcp_flows: 8,
            max_udp_flows: 8,
        },
        mappings: vec![
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
        ],
    }
}

#[test]
fn simulated_outage_shorter_than_grace_returns_to_active_closes_tcp_and_resumes_udp_flow(
) -> TestResult {
    // 1. Bind a real local UDP game server on loopback so UDP flows bind real sockets.
    let local_udp_server = UdpSocket::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    local_udp_server.set_read_timeout(Some(Duration::from_secs(2)))?;
    local_udp_server.set_write_timeout(Some(Duration::from_secs(2)))?;
    let local_udp_addr = local_udp_server.local_addr()?;

    let mut session = EngineTunnelSession::new(
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
        8,
        8,
    )?;

    // 2. Drive Idle -> Connecting -> Authenticating -> Active.
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

    // 3. Verify ping/pong RTT sampling and Degraded -> Active recovery (`NET-005`).
    let ping_1 = session.build_ping(2_000);
    assert_eq!(ping_1, ControlMessage::Ping(SequenceMessage { seq: 1 }));
    // High RTT (400ms > 250ms threshold) transitions Active -> Degraded.
    assert_eq!(
        session.on_pong(SequenceMessage { seq: 1 }, 2_400),
        Some(400)
    );
    assert_eq!(session.state(), EngineSessionState::Degraded);

    // Healthy RTT (30ms <= 250ms) transitions Degraded -> Active.
    let ping_2 = session.build_ping(3_000);
    assert_eq!(ping_2, ControlMessage::Ping(SequenceMessage { seq: 2 }));
    assert_eq!(session.on_pong(SequenceMessage { seq: 2 }, 3_030), Some(30));
    assert_eq!(session.state(), EngineSessionState::Active);

    // 4. Open 1 TCP flow and 1 UDP flow (`flow_id = 77`) before the simulated outage.
    let _ = session
        .tcp
        .evaluate_open_header(&TcpOpenHeader { mapping_index: 0 }.encode())
        .map_err(|(_, e)| e)?;
    assert_eq!(session.tcp.metrics().active_tcp_flows, 1);

    let udp_pkt_before =
        encode_udp_datagram(1, 77, b"pre-outage-game-packet", DEFAULT_MAX_DATAGRAM_BYTES)?;
    session
        .udp
        .handle_relay_datagram(&udp_pkt_before, 4_000)
        .map_err(|e| format!("udp failed: {e:?}"))?;
    assert_eq!(session.udp.metrics().active_udp_flows, 1);
    let local_flow_addr_before = session
        .udp
        .flow_local_addr(1, 77)
        .ok_or("missing flow 77 local addr")?;

    let mut server_buf = [0u8; 512];
    let (n, src_before) = local_udp_server.recv_from(&mut server_buf)?;
    assert_eq!(
        server_buf.get(..n).unwrap_or(&[]),
        b"pre-outage-game-packet"
    );
    assert_eq!(src_before, local_flow_addr_before);

    // 5. Simulate silent network drop lasting 15_000ms (`idle_timeout_ms = 15_000`).
    // At t = 14_000ms (10_970ms since last pong at 3_030ms >= 2 * heartbeat_interval_ms),
    // session enters `Degraded`.
    assert_eq!(
        session.evaluate_liveness(14_000),
        EngineSessionState::Degraded
    );
    // At t = 18_030ms (15_000ms since last pong at 3_030ms >= idle_timeout_ms),
    // session enters `Reconnecting` (`NET-005`).
    assert_eq!(
        session.evaluate_liveness(18_030),
        EngineSessionState::Reconnecting
    );

    // TCP connections are closed immediately on tunnel drop (`contracts.md` §10, `FR-014`),
    // while the UDP flow table and its bound local socket remain intact!
    assert_eq!(session.tcp.metrics().active_tcp_flows, 0);
    assert_eq!(session.udp.metrics().active_udp_flows, 1);
    assert_eq!(
        session.udp.flow_local_addr(1, 77),
        Some(local_flow_addr_before)
    );

    // 6. Reconnect succeeds at t = 20_000ms (within the 30_000ms relay grace window, `NET-004`).
    // Session returns to `Active` with the exact same public ports (`25565` and `19132`).
    let ack_after = sample_ack(SESSION_ID_2);
    assert_eq!(
        session.on_hello_ack(ack_after.clone(), 20_000)?,
        EngineSessionState::Active
    );
    assert_eq!(session.session_id(), Some(SESSION_ID_2));
    assert_eq!(session.negotiated_mappings(), ack_after.mappings.as_slice());

    // 7. Verify UDP flow 77 resumes on the exact same local UDP socket so the game server sees no
    // client port change (`FR-014`, `NET-004`)!
    let udp_pkt_after = encode_udp_datagram(
        1,
        77,
        b"post-outage-resumed-packet",
        DEFAULT_MAX_DATAGRAM_BYTES,
    )?;
    session
        .udp
        .handle_relay_datagram(&udp_pkt_after, 20_100)
        .map_err(|e| format!("post-outage udp failed: {e:?}"))?;

    let (n2, src_after) = local_udp_server.recv_from(&mut server_buf)?;
    assert_eq!(
        server_buf.get(..n2).unwrap_or(&[]),
        b"post-outage-resumed-packet"
    );
    assert_eq!(src_after, local_flow_addr_before);

    local_udp_server.send_to(b"server-ack-after-outage", src_after)?;
    let replies = session.udp.poll_local_replies(4, 20_110);
    assert_eq!(replies.len(), 1);
    if let Some(first) = replies.first() {
        let decoded = UdpDatagram::decode(first, DEFAULT_MAX_DATAGRAM_BYTES)?;
        assert_eq!(decoded.mapping_index, 1);
        assert_eq!(decoded.flow_id, 77);
        assert_eq!(decoded.payload, b"server-ack-after-outage");
    }

    Ok(())
}
