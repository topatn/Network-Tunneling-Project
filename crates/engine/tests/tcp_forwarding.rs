//! Network and security tests for the engine TCP forwarding path (`T-061`, `NET-001`, `NET-003`, `SEC-006`).

use std::error::Error;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use gtt_engine::{engine_handle_tcp_stream, EngineTcpError, EngineTcpSession, LocalMappingConfig};
use gtt_protocol::{sha256_digest, Protocol, TcpOpenHeader, TcpOpenStatus};

type TestResult = Result<(), Box<dyn Error>>;

fn make_deterministic_payload(len: usize, seed: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let b = seed
            .wrapping_add(u8::try_from(i % 251).unwrap_or(0))
            .wrapping_mul(37);
        out.push(b);
    }
    out
}

#[test]
fn tcp_bidirectional_transfer_matches_sha256_and_honors_half_close_both_ways() -> TestResult {
    // 1. Start a local TCP game server on loopback.
    let local_server = TcpListener::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    let local_server_addr = local_server.local_addr()?;

    let guest_to_server_payload = make_deterministic_payload(128 * 1024 + 333, 0x17);
    let server_to_guest_payload = make_deterministic_payload(96 * 1024 + 777, 0xA3);
    let expected_guest_hash = sha256_digest(&guest_to_server_payload);
    let expected_server_hash = sha256_digest(&server_to_guest_payload);

    let server_send_copy = server_to_guest_payload.clone();
    let server_handle = thread::spawn(move || -> Result<[u8; 32], String> {
        let (mut conn, _) = local_server.accept().map_err(|e| e.to_string())?;
        conn.set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| e.to_string())?;
        conn.set_write_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| e.to_string())?;

        // Read all bytes from the guest until guest half-closes (EOF).
        let mut received = Vec::new();
        conn.read_to_end(&mut received).map_err(|e| e.to_string())?;

        // After guest half-close, send the full server reply and close write half.
        conn.write_all(&server_send_copy)
            .map_err(|e| e.to_string())?;
        conn.flush().map_err(|e| e.to_string())?;
        let _ = conn.shutdown(Shutdown::Write);

        Ok(sha256_digest(&received))
    });

    // 2. Start a simulated relay tunnel listener. The host engine connects OUTBOUND to the relay
    // (`NET-003`: host requires zero inbound ports from the relay).
    let relay_tunnel_listener = TcpListener::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    let relay_tunnel_addr = relay_tunnel_listener.local_addr()?;

    let mut engine_session = EngineTcpSession::new(
        vec![LocalMappingConfig::new(
            0,
            "game",
            Protocol::Tcp,
            true,
            local_server_addr,
        )],
        4,
    )?;

    let engine_handle = thread::spawn(move || -> Result<(u64, u64), String> {
        // Outbound connection from host engine to relay.
        let engine_tunnel_stream =
            TcpStream::connect_timeout(&relay_tunnel_addr, Duration::from_secs(5))
                .map_err(|e| e.to_string())?;
        engine_handle_tcp_stream(&mut engine_session, engine_tunnel_stream)
            .map_err(|e| e.to_string())
    });

    let (mut relay_side_stream, _) = relay_tunnel_listener.accept()?;
    relay_side_stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    relay_side_stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    // Relay sends 3-byte TcpOpenHeader for mapping_index 0.
    let open_header = TcpOpenHeader { mapping_index: 0 }.encode();
    relay_side_stream.write_all(&open_header)?;
    relay_side_stream.flush()?;

    // Relay reads 1-byte status (`0x00` Connected).
    let mut status = [0u8; 1];
    relay_side_stream.read_exact(&mut status)?;
    assert_eq!(status, [TcpOpenStatus::Connected.to_byte()]);

    // Guest sends payload through the relay stream and then half-closes write side (`Shutdown::Write`)
    // while continuing to read the server's reply (`NET-001`).
    relay_side_stream.write_all(&guest_to_server_payload)?;
    relay_side_stream.flush()?;
    relay_side_stream.shutdown(Shutdown::Write)?;

    let mut guest_received = Vec::new();
    relay_side_stream.read_to_end(&mut guest_received)?;

    let server_received_hash = server_handle
        .join()
        .map_err(|_| "server thread panicked")??;
    let (bytes_to_local, bytes_from_local) = engine_handle
        .join()
        .map_err(|_| "engine thread panicked")??;

    assert_eq!(server_received_hash, expected_guest_hash);
    assert_eq!(sha256_digest(&guest_received), expected_server_hash);
    assert_eq!(
        bytes_to_local,
        u64::try_from(guest_to_server_payload.len())?
    );
    assert_eq!(
        bytes_from_local,
        u64::try_from(server_to_guest_payload.len())?
    );
    Ok(())
}

#[test]
fn server_initiated_half_close_preserves_guest_to_server_stream() -> TestResult {
    // Verify half-close in the opposite direction: server sends banner and immediately half-closes
    // its write side while still reading the guest's upload to completion (`NET-001`).
    let local_server = TcpListener::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    let local_server_addr = local_server.local_addr()?;

    let banner = make_deterministic_payload(24 * 1024, 0x31);
    let guest_upload = make_deterministic_payload(40 * 1024, 0x59);
    let expected_banner_hash = sha256_digest(&banner);
    let expected_upload_hash = sha256_digest(&guest_upload);

    let server_handle = thread::spawn(move || -> Result<[u8; 32], String> {
        let (mut conn, _) = local_server.accept().map_err(|e| e.to_string())?;
        conn.set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| e.to_string())?;
        conn.set_write_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| e.to_string())?;

        conn.write_all(&banner).map_err(|e| e.to_string())?;
        conn.flush().map_err(|e| e.to_string())?;
        conn.shutdown(Shutdown::Write).map_err(|e| e.to_string())?;

        let mut upload = Vec::new();
        conn.read_to_end(&mut upload).map_err(|e| e.to_string())?;
        Ok(sha256_digest(&upload))
    });

    let relay_listener = TcpListener::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    let relay_addr = relay_listener.local_addr()?;

    let mut engine_session = EngineTcpSession::new(
        vec![LocalMappingConfig::new(
            0,
            "game",
            Protocol::Tcp,
            true,
            local_server_addr,
        )],
        4,
    )?;

    let engine_handle = thread::spawn(move || -> Result<(u64, u64), String> {
        let stream = TcpStream::connect_timeout(&relay_addr, Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        engine_handle_tcp_stream(&mut engine_session, stream).map_err(|e| e.to_string())
    });

    let (mut relay_stream, _) = relay_listener.accept()?;
    relay_stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    relay_stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    relay_stream.write_all(&TcpOpenHeader { mapping_index: 0 }.encode())?;
    let mut status = [0u8; 1];
    relay_stream.read_exact(&mut status)?;
    assert_eq!(status, [TcpOpenStatus::Connected.to_byte()]);

    // Read server banner until server's FIN (`EOF`).
    let mut received_banner = Vec::new();
    relay_stream.read_to_end(&mut received_banner)?;
    assert_eq!(sha256_digest(&received_banner), expected_banner_hash);

    // Even after reading EOF from the server, the guest->server direction is still open!
    relay_stream.write_all(&guest_upload)?;
    relay_stream.flush()?;
    relay_stream.shutdown(Shutdown::Write)?;

    let upload_hash = server_handle
        .join()
        .map_err(|_| "server thread panicked")??;
    let _ = engine_handle
        .join()
        .map_err(|_| "engine thread panicked")??;

    assert_eq!(upload_hash, expected_upload_hash);
    Ok(())
}

#[test]
fn engine_dial_protocol_returns_0x01_refused_and_0x02_limit_reached_correctly() -> TestResult {
    // Bind a port and immediately drop the listener so dialing `closed_addr` is guaranteed to fail.
    let closed_addr = {
        let listener = TcpListener::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
        listener.local_addr()?
    };

    let mut session = EngineTcpSession::new(
        vec![
            LocalMappingConfig::new(0, "game", Protocol::Tcp, true, closed_addr),
            LocalMappingConfig::new(
                1,
                "rcon",
                Protocol::Tcp,
                false,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 25575),
            ),
            LocalMappingConfig::new(
                2,
                "query",
                Protocol::Udp,
                true,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 25565),
            ),
        ],
        1,
    )?;

    // 1. Unknown mapping index (9) -> 0x01 Refused (`SEC-006`).
    let err = session.evaluate_open_header(&TcpOpenHeader { mapping_index: 9 }.encode());
    assert!(matches!(
        err,
        Err((
            TcpOpenStatus::Refused,
            EngineTcpError::RefusedMapping { mapping_index: 9 }
        ))
    ));

    // 2. Disabled optional mapping (index 1, `GAME-008`) -> 0x01 Refused (`SEC-006`).
    let err = session.evaluate_open_header(&TcpOpenHeader { mapping_index: 1 }.encode());
    assert!(matches!(
        err,
        Err((
            TcpOpenStatus::Refused,
            EngineTcpError::RefusedMapping { mapping_index: 1 }
        ))
    ));

    // 3. UDP mapping requested on TCP stream (index 2) -> 0x01 Refused (`SEC-006`).
    let err = session.evaluate_open_header(&TcpOpenHeader { mapping_index: 2 }.encode());
    assert!(matches!(
        err,
        Err((
            TcpOpenStatus::Refused,
            EngineTcpError::RefusedMapping { mapping_index: 2 }
        ))
    ));

    // 4. Valid TCP mapping (index 0) reserves the single slot (`max_tcp_flows = 1`); a second
    // concurrent open returns 0x02 LimitReached.
    let reserved = session.evaluate_open_header(&TcpOpenHeader { mapping_index: 0 }.encode());
    assert!(reserved.is_ok());

    let limit_err = session.evaluate_open_header(&TcpOpenHeader { mapping_index: 0 }.encode());
    assert!(matches!(
        limit_err,
        Err((
            TcpOpenStatus::LimitReached,
            EngineTcpError::LimitReached { max_tcp_flows: 1 }
        ))
    ));

    // Release the slot and verify a real stream to `closed_addr` writes 0x01 Refused on dial failure.
    session.finish_tcp_flow(0, 0);

    let relay_listener = TcpListener::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    let relay_addr = relay_listener.local_addr()?;
    let engine_stream = TcpStream::connect_timeout(&relay_addr, Duration::from_secs(2))?;
    let (mut relay_side, _) = relay_listener.accept()?;
    relay_side.set_read_timeout(Some(Duration::from_secs(2)))?;
    relay_side.set_write_timeout(Some(Duration::from_secs(2)))?;

    relay_side.write_all(&TcpOpenHeader { mapping_index: 0 }.encode())?;
    relay_side.flush()?;

    let dial_res = engine_handle_tcp_stream(&mut session, engine_stream);
    assert!(matches!(
        dial_res,
        Err(EngineTcpError::LocalDialFailed { .. })
    ));

    let mut status = [0u8; 1];
    relay_side.read_exact(&mut status)?;
    assert_eq!(status, [TcpOpenStatus::Refused.to_byte()]);
    assert_eq!(session.metrics().active_tcp_flows, 0);

    Ok(())
}
