//! Network and security tests for the relay TCP forwarding path (`T-061`, `NET-001`, `NET-003`, `SEC-005`).

use std::error::Error;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use gtt_protocol::{sha256_digest, HelloAckMapping, Protocol, TcpOpenHeader, TcpOpenStatus};
use gtt_relay::{relay_forward_tcp_connection, RelayTcpError, RelayTcpSession};

type TestResult = Result<(), Box<dyn Error>>;

fn sample_mappings() -> Vec<HelloAckMapping> {
    vec![
        HelloAckMapping {
            index: 0,
            mapping_key: "game".to_owned(),
            protocol: Protocol::Tcp,
            public_port: 25565,
        },
        HelloAckMapping {
            index: 1,
            mapping_key: "voice".to_owned(),
            protocol: Protocol::Udp,
            public_port: 25566,
        },
    ]
}

#[test]
fn relay_tcp_forwarding_verifies_sha256_both_ways_with_host_inbound_blocked() -> TestResult {
    // Public listener on the relay accepting guest TCP connections, and tunnel stream listener on
    // the relay accepting outbound streams from the host engine (`NET-003`: host has no inbound
    // listener exposed to the relay).
    let public_listener = TcpListener::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    let public_addr = public_listener.local_addr()?;

    let relay_tunnel_listener = TcpListener::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    let relay_tunnel_addr = relay_tunnel_listener.local_addr()?;

    let guest_payload: Vec<u8> = (0..65_537usize)
        .map(|i| u8::try_from((i * 13 + 7) % 256).unwrap_or(0))
        .collect();
    let host_payload: Vec<u8> = (0..49_153usize)
        .map(|i| u8::try_from((i * 29 + 3) % 256).unwrap_or(0))
        .collect();
    let expected_guest_hash = sha256_digest(&guest_payload);
    let expected_host_hash = sha256_digest(&host_payload);

    // Host engine connects OUTBOUND to the relay tunnel listener.
    let host_send_copy = host_payload.clone();
    let host_handle = thread::spawn(move || -> Result<[u8; 32], String> {
        let mut engine_stream =
            TcpStream::connect_timeout(&relay_tunnel_addr, Duration::from_secs(5))
                .map_err(|e| e.to_string())?;
        engine_stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| e.to_string())?;
        engine_stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| e.to_string())?;

        // Read 3-byte TcpOpenHeader from relay.
        let mut hdr = [0u8; 3];
        engine_stream
            .read_exact(&mut hdr)
            .map_err(|e| e.to_string())?;
        let decoded = TcpOpenHeader::decode(&hdr).map_err(|e| e.to_string())?;
        if decoded.mapping_index != 0 {
            return Err(format!(
                "unexpected mapping_index {}",
                decoded.mapping_index
            ));
        }

        // Reply 0x00 Connected.
        engine_stream
            .write_all(&[TcpOpenStatus::Connected.to_byte()])
            .map_err(|e| e.to_string())?;
        engine_stream.flush().map_err(|e| e.to_string())?;

        // Read guest payload until guest half-closes (EOF).
        let mut from_guest = Vec::new();
        engine_stream
            .read_to_end(&mut from_guest)
            .map_err(|e| e.to_string())?;

        // Write host payload and half-close.
        engine_stream
            .write_all(&host_send_copy)
            .map_err(|e| e.to_string())?;
        engine_stream.flush().map_err(|e| e.to_string())?;
        let _ = engine_stream.shutdown(Shutdown::Write);

        Ok(sha256_digest(&from_guest))
    });

    // Guest connects inbound to the relay's public port.
    let guest_send_copy = guest_payload.clone();
    let guest_handle = thread::spawn(move || -> Result<[u8; 32], String> {
        let mut guest_conn = TcpStream::connect_timeout(&public_addr, Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        guest_conn
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| e.to_string())?;
        guest_conn
            .set_write_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| e.to_string())?;

        guest_conn
            .write_all(&guest_send_copy)
            .map_err(|e| e.to_string())?;
        guest_conn.flush().map_err(|e| e.to_string())?;
        guest_conn
            .shutdown(Shutdown::Write)
            .map_err(|e| e.to_string())?;

        let mut from_host = Vec::new();
        guest_conn
            .read_to_end(&mut from_host)
            .map_err(|e| e.to_string())?;
        Ok(sha256_digest(&from_host))
    });

    let (guest_side_stream, _) = public_listener.accept()?;
    let (relay_tunnel_stream, _) = relay_tunnel_listener.accept()?;

    let mut relay_session = RelayTcpSession::new(&sample_mappings(), 8);
    let (g2e, e2g) = relay_forward_tcp_connection(
        &mut relay_session,
        0,
        guest_side_stream,
        relay_tunnel_stream,
        Duration::from_secs(5),
    )?;

    let host_received_hash = host_handle.join().map_err(|_| "host thread panicked")??;
    let guest_received_hash = guest_handle.join().map_err(|_| "guest thread panicked")??;

    assert_eq!(host_received_hash, expected_guest_hash);
    assert_eq!(guest_received_hash, expected_host_hash);
    assert_eq!(g2e, u64::try_from(guest_payload.len())?);
    assert_eq!(e2g, u64::try_from(host_payload.len())?);
    assert_eq!(relay_session.metrics().active_tcp_flows, 0);
    assert_eq!(relay_session.metrics().total_connected, 1);
    assert_eq!(relay_session.distinct_source_count(), 1);

    Ok(())
}

#[test]
fn relay_tcp_session_enforces_mapping_protocol_flow_caps_and_engine_reject_statuses() -> TestResult
{
    let mut session = RelayTcpSession::new(&sample_mappings(), 1);
    let guest_ip = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 10));

    // 1. UDP mapping index (1) or unknown index (5) is refused immediately.
    assert!(matches!(
        session.begin_tcp_open(1, guest_ip),
        Err(RelayTcpError::InvalidTcpMapping { mapping_index: 1 })
    ));
    assert!(matches!(
        session.begin_tcp_open(5, guest_ip),
        Err(RelayTcpError::InvalidTcpMapping { mapping_index: 5 })
    ));
    assert_eq!(session.metrics().total_refused, 2);

    // 2. Valid TCP mapping (0) reserves the 1 available flow slot; a second concurrent open hits
    // FlowLimitReached.
    let hdr = session.begin_tcp_open(0, guest_ip)?;
    assert_eq!(hdr, [0x01, 0x00, 0x00]);
    assert_eq!(session.metrics().active_tcp_flows, 1);

    assert!(matches!(
        session.begin_tcp_open(0, guest_ip),
        Err(RelayTcpError::FlowLimitReached { max_tcp_flows: 1 })
    ));
    assert_eq!(session.metrics().total_limit_reached, 1);

    // 3. If engine replies 0x01 Refused, the flow slot is freed and total_refused increments.
    assert!(matches!(
        session.complete_tcp_open(0x01),
        Err(RelayTcpError::EngineRejected(TcpOpenStatus::Refused))
    ));
    assert_eq!(session.metrics().active_tcp_flows, 0);
    assert_eq!(session.metrics().total_refused, 3);

    // 4. If engine replies 0x02 LimitReached, the flow slot is freed and total_limit_reached increments.
    let _ = session.begin_tcp_open(0, guest_ip)?;
    assert!(matches!(
        session.complete_tcp_open(0x02),
        Err(RelayTcpError::EngineRejected(TcpOpenStatus::LimitReached))
    ));
    assert_eq!(session.metrics().active_tcp_flows, 0);
    assert_eq!(session.metrics().total_limit_reached, 2);

    // 5. SEC-005 source audit across all relay source files:
    // confirm no outbound connect calls exist in the relay data path.
    for src in [
        include_str!("../src/lib.rs"),
        include_str!("../src/tcp.rs"),
        include_str!("../src/udp.rs"),
        include_str!("../src/session.rs"),
        include_str!("../src/limits.rs"),
        include_str!("../src/shutdown.rs"),
    ] {
        for forbidden in [
            "TcpStream::connect",
            "UdpSocket::connect",
            "connect_timeout",
        ] {
            assert!(
                !src.contains(forbidden),
                "SEC-005 violation: relay source must not contain outbound dial API {forbidden}"
            );
        }
    }

    Ok(())
}
