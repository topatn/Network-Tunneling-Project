//! Network and security tests for the engine UDP forwarding path (`T-062`, `NET-002`, `NET-007`, `SEC-006`).

use std::error::Error;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use gtt_engine::{EngineUdpDropReason, EngineUdpSession, LocalMappingConfig};
use gtt_protocol::{encode_udp_datagram, Protocol, UdpDatagram, DEFAULT_MAX_DATAGRAM_BYTES};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn two_udp_flows_bind_distinct_local_sockets_and_receive_only_their_own_replies() -> TestResult {
    // 1. Bind a real local UDP game server on loopback.
    let local_server = UdpSocket::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    local_server.set_read_timeout(Some(Duration::from_secs(2)))?;
    local_server.set_write_timeout(Some(Duration::from_secs(2)))?;
    let local_server_addr = local_server.local_addr()?;

    let mut engine_udp = EngineUdpSession::new(
        vec![
            LocalMappingConfig::new(
                0,
                "tcp-main",
                Protocol::Tcp,
                true,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 25565),
            ),
            LocalMappingConfig::new(1, "bedrock-udp", Protocol::Udp, true, local_server_addr),
        ],
        4,
        DEFAULT_MAX_DATAGRAM_BYTES,
        60_000,
    )?;

    // 2. Relay sends two datagrams for mapping_index 1 from two distinct guest sources
    // (`flow_id = 10` for Guest A, `flow_id = 20` for Guest B).
    let guest_a_ping = b"guest-A-unconnected-ping-seq-101";
    let guest_b_ping = b"guest-B-unconnected-ping-seq-202";

    let frame_a = encode_udp_datagram(1, 10, guest_a_ping, DEFAULT_MAX_DATAGRAM_BYTES)?;
    let frame_b = encode_udp_datagram(1, 20, guest_b_ping, DEFAULT_MAX_DATAGRAM_BYTES)?;

    engine_udp
        .handle_relay_datagram(&frame_a, 1_000)
        .map_err(|e| format!("frame_a failed: {e:?}"))?;
    engine_udp
        .handle_relay_datagram(&frame_b, 1_000)
        .map_err(|e| format!("frame_b failed: {e:?}"))?;

    // Verify per-flow local UDP sockets are distinct (`contracts.md` §11).
    let addr_flow_a = engine_udp
        .flow_local_addr(1, 10)
        .ok_or("missing flow_local_addr for flow 10")?;
    let addr_flow_b = engine_udp
        .flow_local_addr(1, 20)
        .ok_or("missing flow_local_addr for flow 20")?;
    assert_ne!(
        addr_flow_a, addr_flow_b,
        "each UDP flow must bind its own distinct local UDP socket"
    );

    // 3. Local UDP game server receives both packets from the two distinct ephemeral local ports.
    let mut buf = [0u8; 2048];
    let (len_1, src_1) = local_server.recv_from(&mut buf)?;
    assert_eq!(buf.get(..len_1).unwrap_or(&[]), guest_a_ping);
    assert_eq!(src_1, addr_flow_a);

    let (len_2, src_2) = local_server.recv_from(&mut buf)?;
    assert_eq!(buf.get(..len_2).unwrap_or(&[]), guest_b_ping);
    assert_eq!(src_2, addr_flow_b);

    // 4. Local server replies in REVERSE order (to Guest B first, then Guest A) to prove that
    // reply routing is determined strictly by the per-flow socket, not arrival order (`NET-002`).
    let reply_for_b = b"server-pong-exclusively-for-guest-B";
    let reply_for_a = b"server-pong-exclusively-for-guest-A";
    local_server.send_to(reply_for_b, src_2)?;
    local_server.send_to(reply_for_a, src_1)?;

    let replies = engine_udp.poll_local_replies(8, 1_010);
    assert_eq!(replies.len(), 2);

    let mut received_for_a = Vec::new();
    let mut received_for_b = Vec::new();
    for frame in &replies {
        let decoded = UdpDatagram::decode(frame, DEFAULT_MAX_DATAGRAM_BYTES)?;
        assert_eq!(decoded.mapping_index, 1);
        match decoded.flow_id {
            10 => received_for_a.push(decoded.payload.to_vec()),
            20 => received_for_b.push(decoded.payload.to_vec()),
            other => return Err(format!("unexpected flow_id {other}").into()),
        }
    }

    assert_eq!(received_for_a, vec![reply_for_a.to_vec()]);
    assert_eq!(received_for_b, vec![reply_for_b.to_vec()]);
    assert_eq!(engine_udp.metrics().datagrams_to_local, 2);
    assert_eq!(engine_udp.metrics().datagrams_from_local, 2);
    assert_eq!(engine_udp.metrics().datagrams_dropped, 0);

    Ok(())
}

#[test]
fn oversize_unknown_mapping_malformed_and_excess_flows_are_dropped_and_counted_and_flows_expire(
) -> TestResult {
    let local_server = UdpSocket::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    let local_server_addr = local_server.local_addr()?;

    let mut engine_udp = EngineUdpSession::new(
        vec![
            LocalMappingConfig::new(
                0,
                "tcp-only",
                Protocol::Tcp,
                true,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 25565),
            ),
            LocalMappingConfig::new(1, "udp-main", Protocol::Udp, true, local_server_addr),
            LocalMappingConfig::new(2, "udp-disabled", Protocol::Udp, false, local_server_addr),
        ],
        2,   // max_udp_flows = 2
        100, // max_datagram_bytes = 100 (usable payload = 94)
        5_000,
    )?;

    // 1. Truncated frame (< 6 bytes) -> dropped_malformed (`NET-007`).
    let res = engine_udp.handle_relay_datagram(&[0x00, 0x01, 0x00], 1_000);
    assert_eq!(res, Err(EngineUdpDropReason::MalformedHeader));
    assert_eq!(engine_udp.metrics().datagrams_dropped, 1);
    assert_eq!(engine_udp.metrics().dropped_malformed, 1);

    // 2. Oversized datagram (> 100 bytes total) -> dropped_oversize (`NET-007`).
    let mut oversized_raw = vec![0u8; 101];
    if let Some(b) = oversized_raw.get_mut(1) {
        *b = 1;
    }
    if let Some(b) = oversized_raw.get_mut(5) {
        *b = 1;
    }
    let res = engine_udp.handle_relay_datagram(&oversized_raw, 1_000);
    assert_eq!(res, Err(EngineUdpDropReason::OversizedDatagram));
    assert_eq!(engine_udp.metrics().datagrams_dropped, 2);
    assert_eq!(engine_udp.metrics().dropped_oversize, 1);

    // 3. Unknown mapping_index (99), TCP mapping_index (0), and disabled UDP mapping_index (2)
    // -> dropped_unknown_mapping (`NET-007`, `SEC-006`).
    for bad_idx in [99u16, 0u16, 2u16] {
        let frame = encode_udp_datagram(bad_idx, 1, b"hello", 100)?;
        let res = engine_udp.handle_relay_datagram(&frame, 1_000);
        assert_eq!(res, Err(EngineUdpDropReason::UnknownOrDisabledMapping));
    }
    assert_eq!(engine_udp.metrics().datagrams_dropped, 5);
    assert_eq!(engine_udp.metrics().dropped_unknown_mapping, 3);

    // 4. Open 2 valid UDP flows (reaching max_udp_flows = 2); a 3rd flow is dropped and counted.
    let f1 = encode_udp_datagram(1, 1, b"flow-1", 100)?;
    let f2 = encode_udp_datagram(1, 2, b"flow-2", 100)?;
    let f3 = encode_udp_datagram(1, 3, b"flow-3", 100)?;
    assert!(engine_udp.handle_relay_datagram(&f1, 1_000).is_ok());
    assert!(engine_udp.handle_relay_datagram(&f2, 2_000).is_ok());
    assert_eq!(engine_udp.metrics().active_udp_flows, 2);

    let res = engine_udp.handle_relay_datagram(&f3, 2_500);
    assert_eq!(res, Err(EngineUdpDropReason::FlowLimitReached));
    assert_eq!(engine_udp.metrics().datagrams_dropped, 6);
    assert_eq!(engine_udp.metrics().dropped_flow_limit, 1);

    // 5. At t = 6_000ms, flow 1 (last active at 1_000ms, timeout = 5_000ms) expires while flow 2
    // (last active at 2_000ms) stays active; flow 3 is now admitted!
    assert!(engine_udp.handle_relay_datagram(&f3, 6_000).is_ok());
    assert_eq!(engine_udp.metrics().flows_expired, 1);
    assert_eq!(engine_udp.metrics().active_udp_flows, 2);

    Ok(())
}
