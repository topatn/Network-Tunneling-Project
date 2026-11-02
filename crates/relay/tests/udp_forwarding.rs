//! Network and security tests for the relay UDP forwarding path (`T-062`, `NET-002`, `NET-007`).

use std::error::Error;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use gtt_protocol::{encode_udp_datagram, HelloAckMapping, Protocol, UdpDatagram};
use gtt_relay::RelayUdpSession;

type TestResult = Result<(), Box<dyn Error>>;

fn sample_mappings() -> Vec<HelloAckMapping> {
    vec![
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
    ]
}

#[test]
fn two_guest_udp_sockets_receive_only_their_own_replies_over_relay_udp_session() -> TestResult {
    // Bind two distinct guest UDP sockets and one relay public UDP listener socket on loopback.
    let guest_a = UdpSocket::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    let guest_b = UdpSocket::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    let relay_public = UdpSocket::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;

    for sock in [&guest_a, &guest_b, &relay_public] {
        sock.set_read_timeout(Some(Duration::from_secs(2)))?;
        sock.set_write_timeout(Some(Duration::from_secs(2)))?;
    }

    let relay_public_addr = relay_public.local_addr()?;
    let guest_a_addr = guest_a.local_addr()?;
    let guest_b_addr = guest_b.local_addr()?;

    let mut relay_udp = RelayUdpSession::new(&sample_mappings(), 8, 1200, 30_000);

    // 1. Guest A and Guest B send packets to the relay's public UDP port (`mapping_index = 1`).
    let pkt_a1 = b"guest-A-packet-1";
    let pkt_b1 = b"guest-B-packet-1";
    let pkt_a2 = b"guest-A-packet-2";

    guest_a.send_to(pkt_a1, relay_public_addr)?;
    guest_b.send_to(pkt_b1, relay_public_addr)?;
    guest_a.send_to(pkt_a2, relay_public_addr)?;

    let mut buf = [0u8; 1500];

    let (n1, src1) = relay_public.recv_from(&mut buf)?;
    assert_eq!(src1, guest_a_addr);
    let framed_a1 = relay_udp
        .handle_guest_datagram(1, src1, buf.get(..n1).unwrap_or(&[]), 1_000)
        .ok_or("expected framed_a1")?;

    let (n2, src2) = relay_public.recv_from(&mut buf)?;
    assert_eq!(src2, guest_b_addr);
    let framed_b1 = relay_udp
        .handle_guest_datagram(1, src2, buf.get(..n2).unwrap_or(&[]), 1_005)
        .ok_or("expected framed_b1")?;

    let (n3, src3) = relay_public.recv_from(&mut buf)?;
    assert_eq!(src3, guest_a_addr);
    let framed_a2 = relay_udp
        .handle_guest_datagram(1, src3, buf.get(..n3).unwrap_or(&[]), 1_010)
        .ok_or("expected framed_a2")?;

    let dec_a1 = UdpDatagram::decode(&framed_a1, 1200)?;
    let dec_b1 = UdpDatagram::decode(&framed_b1, 1200)?;
    let dec_a2 = UdpDatagram::decode(&framed_a2, 1200)?;

    // Guest A reuses its existing flow_id; Guest B gets a distinct flow_id (`NET-002`).
    assert_eq!(dec_a1.flow_id, dec_a2.flow_id);
    assert_ne!(dec_a1.flow_id, dec_b1.flow_id);
    assert_eq!(dec_a1.payload, pkt_a1);
    assert_eq!(dec_b1.payload, pkt_b1);
    assert_eq!(dec_a2.payload, pkt_a2);

    // 2. Engine sends replies for Guest B first, then Guest A. Relay routes each reply back to the
    // exact guest socket (`NET-002`).
    let reply_b = b"server-reply-for-guest-B-only";
    let reply_a = b"server-reply-for-guest-A-only";

    let engine_frame_b = encode_udp_datagram(1, dec_b1.flow_id, reply_b, 1200)?;
    let engine_frame_a = encode_udp_datagram(1, dec_a1.flow_id, reply_a, 1200)?;

    let (map_b, dst_b, payload_b) = relay_udp
        .handle_engine_datagram(&engine_frame_b, 1_020)
        .ok_or("expected routed reply_b")?;
    assert_eq!(map_b, 1);
    assert_eq!(dst_b, guest_b_addr);
    relay_public.send_to(&payload_b, dst_b)?;

    let (map_a, dst_a, payload_a) = relay_udp
        .handle_engine_datagram(&engine_frame_a, 1_025)
        .ok_or("expected routed reply_a")?;
    assert_eq!(map_a, 1);
    assert_eq!(dst_a, guest_a_addr);
    relay_public.send_to(&payload_a, dst_a)?;

    // 3. Verify Guest A receives only `reply_a` and Guest B receives only `reply_b`.
    let (len_a, from_a) = guest_a.recv_from(&mut buf)?;
    assert_eq!(from_a, relay_public_addr);
    assert_eq!(buf.get(..len_a).unwrap_or(&[]), reply_a);

    let (len_b, from_b) = guest_b.recv_from(&mut buf)?;
    assert_eq!(from_b, relay_public_addr);
    assert_eq!(buf.get(..len_b).unwrap_or(&[]), reply_b);

    assert_eq!(relay_udp.metrics().active_udp_flows, 2);
    assert_eq!(relay_udp.metrics().datagrams_guest_to_engine, 3);
    assert_eq!(relay_udp.metrics().datagrams_engine_to_guest, 2);
    assert_eq!(relay_udp.metrics().datagrams_dropped, 0);

    Ok(())
}

#[test]
fn relay_udp_drops_and_counts_oversize_unknown_mapping_malformed_and_flow_limit_datagrams(
) -> TestResult {
    // max_udp_flows = 2, max_datagram_bytes = 64 (usable payload = 58 bytes), idle timeout = 5_000ms.
    let mut relay_udp = RelayUdpSession::new(&sample_mappings(), 2, 64, 5_000);
    let guest_1 = SocketAddr::new(Ipv4Addr::new(198, 51, 100, 1).into(), 50001);
    let guest_2 = SocketAddr::new(Ipv4Addr::new(198, 51, 100, 2).into(), 50002);
    let guest_3 = SocketAddr::new(Ipv4Addr::new(198, 51, 100, 3).into(), 50003);

    // 1. Oversize guest datagram (59-byte payload + 6-byte header = 65 > 64) -> dropped and counted (`NET-007`).
    let oversize_payload = [0xAAu8; 59];
    assert!(relay_udp
        .handle_guest_datagram(1, guest_1, &oversize_payload, 1_000)
        .is_none());
    assert_eq!(relay_udp.metrics().datagrams_dropped, 1);
    assert_eq!(relay_udp.metrics().dropped_oversize, 1);
    assert_eq!(relay_udp.metrics().active_udp_flows, 0);

    // 2. Exact boundary payload (58-byte payload + 6-byte header = 64 == max_datagram_bytes) succeeds.
    let exact_payload = [0xBBu8; 58];
    assert!(relay_udp
        .handle_guest_datagram(1, guest_1, &exact_payload, 1_000)
        .is_some());
    assert_eq!(relay_udp.metrics().active_udp_flows, 1);

    // 3. Unknown mapping index (7) and TCP mapping index (0) -> dropped and counted (`NET-007`).
    assert!(relay_udp
        .handle_guest_datagram(7, guest_1, b"bad-map", 1_000)
        .is_none());
    assert!(relay_udp
        .handle_guest_datagram(0, guest_1, b"tcp-map", 1_000)
        .is_none());
    assert_eq!(relay_udp.metrics().datagrams_dropped, 3);
    assert_eq!(relay_udp.metrics().dropped_unknown_mapping, 2);

    // 4. Admit guest_2 (reaching max_udp_flows = 2); guest_3 is dropped and counted (`dropped_flow_limit`).
    assert!(relay_udp
        .handle_guest_datagram(1, guest_2, b"ok", 2_000)
        .is_some());
    assert_eq!(relay_udp.metrics().active_udp_flows, 2);

    assert!(relay_udp
        .handle_guest_datagram(1, guest_3, b"over-limit", 2_500)
        .is_none());
    assert_eq!(relay_udp.metrics().datagrams_dropped, 4);
    assert_eq!(relay_udp.metrics().dropped_flow_limit, 1);

    // 5. Engine -> Relay malformed (< 6 bytes), oversized (> 64 bytes), unknown mapping index, and
    // unknown flow_id frames are all dropped and counted (`NET-007`).
    assert!(relay_udp
        .handle_engine_datagram(&[0x00, 0x01, 0x02], 2_500)
        .is_none());
    assert_eq!(relay_udp.metrics().dropped_malformed, 1);

    let oversize_engine_frame = [0u8; 65];
    assert!(relay_udp
        .handle_engine_datagram(&oversize_engine_frame, 2_500)
        .is_none());
    assert_eq!(relay_udp.metrics().dropped_oversize, 2);

    let unknown_map_frame = encode_udp_datagram(9, 1, b"hi", 64)?;
    assert!(relay_udp
        .handle_engine_datagram(&unknown_map_frame, 2_500)
        .is_none());
    assert_eq!(relay_udp.metrics().dropped_unknown_mapping, 3);

    let unknown_flow_frame = encode_udp_datagram(1, 999, b"hi", 64)?;
    assert!(relay_udp
        .handle_engine_datagram(&unknown_flow_frame, 2_500)
        .is_none());
    assert_eq!(relay_udp.metrics().dropped_unknown_flow, 1);
    assert_eq!(relay_udp.metrics().datagrams_dropped, 8);

    // 6. Idle flow expiry at t = 6_000ms evicts guest_1 (idle since 1_000ms) and admits guest_3.
    assert!(relay_udp
        .handle_guest_datagram(1, guest_3, b"now-admitted", 6_000)
        .is_some());
    assert_eq!(relay_udp.metrics().flows_expired, 1);
    assert_eq!(relay_udp.metrics().active_udp_flows, 2);

    Ok(())
}
