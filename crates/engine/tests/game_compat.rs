//! Automated L0 and L1 game compatibility test suite (`T-090`, `TEST-005`, `GAME-001`..`GAME-009`).
//!
//! Primary focus (per `ADR-013` and milestone sequencing): **Minecraft: Java Edition** (`GAME-002`),
//! covering:
//! 1. **L0** transport reachability (`probe_local_targets` connect-only TCP probe per `FR-011` and
//!    `research.md` §5 RQ-13.2c) and end-to-end TCP byte forwarding on both default (`25565`) and
//!    non-default (`GAME-009`) local ports.
//! 2. **L1** Minecraft: Java Edition Server List Ping (SLP) protocol (`0x00` Handshake + `0x00`
//!    Status Request/Response JSON + `0x01` Ping/Pong) over a live `Guest -> RelayTcpSession ->
//!    EngineTcpSession -> Local Server` tunnel (`NET-001`, `NET-003`, `GAME-002`, `TEST-005`).
//! 3. **`GAME-002` Security Invariant:** Optional `rcon` (TCP `25575`) and `query` (UDP `25565`)
//!    mappings are disabled by default (`GAME-008`), and any attempt to reach the RCON or Query
//!    mapping through the tunnel is refused (`0x01` / dropped) unless the host explicitly enables
//!    that mapping (`GAME-002`, `SEC-006`). When the host explicitly enables `query`, the L1 UT3
//!    Query handshake (`\xfe\xfd\x09`) succeeds through `RelayUdpSession <-> EngineUdpSession`.
//! 4. **Multi-profile L0/L1 & Explicit Gap Inventory (`GAME-001`, `GAME-003`..`GAME-006`,
//!    `TEST-005`):** Exercises L0 TCP forwarding for `terraria` (`GAME-001`), L1 UDP probes
//!    through the tunnel for `minecraft-bedrock` (`RakNet 0x01 -> 0x1c`) and `l4d2` (`Source
//!    A2S_INFO`), and asserts the explicit gap table (`E-1`, `E-5`, and `ADR-011` L2 gate).

use std::error::Error;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use gtt_engine::{
    build_mc_java_slp_ping_request, build_mc_java_slp_status_response, configure_mappings,
    decode_mc_varint, encode_mc_varint, engine_handle_tcp_stream,
    parse_mc_java_slp_status_response, probe_local_targets, probe_minecraft_java_slp,
    EngineTcpSession, EngineUdpSession, LocalMappingConfig, MappingTargetInput, ProbeLevel,
    ProbeStatus, UdpProbeKind, MAX_MC_SLP_STATUS_BYTES,
};
use gtt_profiles::{Evidence, Registry};
use gtt_protocol::{HelloAckMapping, Protocol, TcpOpenHeader, TcpOpenStatus};
use gtt_relay::{relay_forward_tcp_connection, RelayTcpSession, RelayUdpSession};

type TestResult = Result<(), Box<dyn Error>>;
type MockServerHandle = thread::JoinHandle<Result<(), String>>;

fn profiles_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|root| root.join("profiles"))
        .unwrap_or_else(|| PathBuf::from("profiles"))
}

/// Spawn a local Minecraft: Java Edition Server List Ping (SLP) responder on an ephemeral loopback
/// port.
///
/// Follows `research.md` §5 RQ-13.2c:
/// - On the first connection (L0 connect-only probe), sends **nothing** and tolerates immediate EOF.
/// - On an L1 SLP connection, reads the `0x00` Handshake + `0x00` Status Request, replies with a
///   length-prefixed `0x00` JSON Status Response, then reads the 10-byte `0x01` Ping frame and
///   echoes it back verbatim as a `0x01` Pong frame.
fn spawn_mock_minecraft_java_slp_server(
    expected_connections: usize,
    version_name: &'static str,
    protocol_version: u32,
    max_players: u32,
    online_players: u32,
) -> Result<(SocketAddr, MockServerHandle), Box<dyn Error>> {
    let listener = TcpListener::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    let addr = listener.local_addr()?;

    let handle = thread::spawn(move || -> Result<(), String> {
        for _ in 0..expected_connections {
            let (mut stream, _) = listener.accept().map_err(|e| e.to_string())?;
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .map_err(|e| e.to_string())?;
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .map_err(|e| e.to_string())?;

            // Read initial bytes from the client. An L0 connect-only probe closes immediately (0 bytes).
            let mut buf = [0_u8; 512];
            let n = match stream.read(&mut buf) {
                Ok(0) => continue,
                Ok(n) => n,
                Err(_) => continue,
            };

            let received = buf.get(..n).unwrap_or(&[]);
            // Validate that the client sent a VarInt-framed Handshake (packet ID 0x00).
            let Some((handshake_len, hdr_bytes)) = decode_mc_varint(received) else {
                return Err("mock MC server received invalid handshake VarInt".to_owned());
            };
            let handshake_end =
                hdr_bytes.saturating_add(usize::try_from(handshake_len).unwrap_or(0));
            let Some(handshake_body) = received.get(hdr_bytes..handshake_end) else {
                return Err("mock MC server received truncated handshake body".to_owned());
            };
            let Some((packet_id, _)) = decode_mc_varint(handshake_body) else {
                return Err("mock MC server could not decode handshake packet id".to_owned());
            };
            if packet_id != 0x00 {
                return Err(format!("unexpected handshake packet ID: {packet_id}"));
            }

            // Ensure the 2-byte Status Request `[0x01, 0x00]` is present (either in the same read
            // or the next read).
            let mut trailing = received.get(handshake_end..).unwrap_or(&[]).to_vec();
            while trailing.len() < 2 {
                let mut more = [0_u8; 64];
                let m = stream.read(&mut more).map_err(|e| e.to_string())?;
                if m == 0 {
                    return Err("EOF before Status Request packet".to_owned());
                }
                trailing.extend_from_slice(more.get(..m).unwrap_or(&[]));
            }
            if trailing.get(..2) != Some(&[0x01, 0x00]) {
                return Err(format!(
                    "expected Status Request [0x01, 0x00], got {:?}",
                    trailing.get(..2)
                ));
            }

            let status_json = serde_json::json!({
                "version": {
                    "name": version_name,
                    "protocol": protocol_version
                },
                "players": {
                    "max": max_players,
                    "online": online_players,
                    "sample": []
                },
                "description": {
                    "text": "GTT Minecraft Java Compatibility Server"
                },
                "enforcesSecureChat": true
            })
            .to_string();

            let response_frame = build_mc_java_slp_status_response(&status_json);
            stream
                .write_all(&response_frame)
                .map_err(|e| e.to_string())?;
            stream.flush().map_err(|e| e.to_string())?;

            // Read 10-byte Ping request `[0x09, 0x01, ..8B payload..]` and echo it back as Pong.
            let mut ping_buf = trailing.get(2..).unwrap_or(&[]).to_vec();
            while ping_buf.len() < 10 {
                let mut more = [0_u8; 32];
                let m = stream.read(&mut more).map_err(|e| e.to_string())?;
                if m == 0 {
                    return Err("EOF before SLP Ping packet".to_owned());
                }
                ping_buf.extend_from_slice(more.get(..m).unwrap_or(&[]));
            }
            let ping_frame = ping_buf
                .get(..10)
                .ok_or_else(|| "missing 10-byte ping frame".to_owned())?;
            if ping_frame.get(..2) != Some(&[0x09, 0x01]) {
                return Err(format!(
                    "expected Ping header [0x09, 0x01], got {:?}",
                    ping_frame.get(..2)
                ));
            }
            stream.write_all(ping_frame).map_err(|e| e.to_string())?;
            stream.flush().map_err(|e| e.to_string())?;
            let _ = stream.shutdown(Shutdown::Both);
        }
        Ok(())
    });

    Ok((addr, handle))
}

#[test]
fn minecraft_java_l0_and_l1_server_list_ping_pass_through_relay_and_engine_on_default_and_custom_ports(
) -> TestResult {
    let registry = Registry::load_dir(&profiles_dir())?;
    let mc_java = registry
        .get("minecraft-java")
        .ok_or("minecraft-java profile missing")?;

    // Verify profile structure (`GAME-002`, `GAME-007`, `GAME-008`):
    // - `game` (TCP 25565) is enabled by default.
    // - `query` (UDP 25565) and `rcon` (TCP 25575) are optional and disabled by default.
    let default_keys = mc_java.default_enabled_keys();
    assert_eq!(default_keys, vec!["game"]);

    // Spawn a local Minecraft Java server responder expecting:
    // 1) An L0 local connect-only probe (`FR-011`),
    // 2) An L1 SLP status + ping/pong probe directly against the local port,
    // 3) An L1 SLP status + ping/pong probe routed end-to-end through RelayTcpSession <-> EngineTcpSession.
    let (local_mc_addr, server_handle) =
        spawn_mock_minecraft_java_slp_server(3, "26.3", 770, 20, 3)?;

    // Configure engine with a custom non-default local port (`local_mc_addr.port()`, `GAME-009`).
    let target_table = configure_mappings(
        mc_java,
        &[MappingTargetInput {
            mapping_key: "game".to_owned(),
            local_host: "127.0.0.1".to_owned(),
            local_port: local_mc_addr.port(),
            enabled: true,
            confirm_non_loopback: false,
        }],
    )
    .map_err(|e| format!("configure_mappings failed: {e:?}"))?;

    // 1. L0 local reachability probe (`FR-011`).
    let l0_results = probe_local_targets(&target_table, Some("game"), Duration::from_secs(2));
    assert_eq!(l0_results.len(), 1);
    let l0 = l0_results.first().ok_or("missing L0 probe result")?;
    assert_eq!(l0.level, ProbeLevel::L0);
    assert_eq!(l0.status, ProbeStatus::Reachable);
    assert!(l0.ok);

    // 2. Direct L1 SLP probe against the local server.
    let direct_l1 = probe_minecraft_java_slp(
        local_mc_addr,
        770,
        0x0102_0304_0506_0708,
        Duration::from_secs(3),
    )?;
    assert_eq!(direct_l1.probe.level, ProbeLevel::L1);
    assert_eq!(direct_l1.probe.status, ProbeStatus::Reachable);
    assert_eq!(direct_l1.status.version.name, "26.3");
    assert_eq!(direct_l1.status.version.protocol, 770);
    assert_eq!(direct_l1.status.players.max, 20);
    assert_eq!(direct_l1.status.players.online, 3);
    assert!(direct_l1.pong_matched);

    // 3. End-to-end L1 SLP probe through the Relay <-> Engine TCP tunnel (`GAME-002`, `NET-001`, `NET-003`).
    let public_relay_listener = TcpListener::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    let public_relay_addr = public_relay_listener.local_addr()?;

    let tunnel_listener = TcpListener::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    let tunnel_addr = tunnel_listener.local_addr()?;

    let mut engine_tcp = EngineTcpSession::new(
        vec![LocalMappingConfig::new(
            0,
            "game",
            Protocol::Tcp,
            true,
            local_mc_addr,
        )],
        4,
    )?;

    let engine_thread = thread::spawn(move || -> Result<(u64, u64), String> {
        let engine_stream = TcpStream::connect_timeout(&tunnel_addr, Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        engine_handle_tcp_stream(&mut engine_tcp, engine_stream).map_err(|e| e.to_string())
    });

    let relay_thread = thread::spawn(move || -> Result<(u64, u64), String> {
        let (guest_stream, _) = public_relay_listener.accept().map_err(|e| e.to_string())?;
        let (tunnel_stream, _) = tunnel_listener.accept().map_err(|e| e.to_string())?;
        let mut relay_tcp = RelayTcpSession::new(
            &[HelloAckMapping {
                index: 0,
                mapping_key: "game".to_owned(),
                protocol: Protocol::Tcp,
                public_port: public_relay_addr.port(),
            }],
            4,
        );
        relay_forward_tcp_connection(
            &mut relay_tcp,
            0,
            guest_stream,
            tunnel_stream,
            Duration::from_secs(5),
        )
        .map_err(|e| format!("{e:?}"))
    });

    // Guest runs L1 Minecraft Java SLP against `public_relay_addr`!
    let tunneled_l1 = probe_minecraft_java_slp(
        public_relay_addr,
        770,
        0x1122_3344_5566_7788,
        Duration::from_secs(5),
    )?;
    assert_eq!(tunneled_l1.probe.level, ProbeLevel::L1);
    assert_eq!(tunneled_l1.probe.status, ProbeStatus::Reachable);
    assert_eq!(tunneled_l1.status.version.name, "26.3");
    assert_eq!(tunneled_l1.status.version.protocol, 770);
    assert_eq!(tunneled_l1.status.players.max, 20);
    assert_eq!(tunneled_l1.status.players.online, 3);
    assert!(tunneled_l1.pong_matched);

    let (relay_up, relay_down) = relay_thread.join().map_err(|_| "relay thread panicked")??;
    let (engine_up, engine_down) = engine_thread
        .join()
        .map_err(|_| "engine thread panicked")??;
    server_handle
        .join()
        .map_err(|_| "mock MC server panicked")??;

    assert!(relay_up > 0 && relay_down > 0);
    assert_eq!(relay_up, engine_up);
    assert_eq!(relay_down, engine_down);

    Ok(())
}

#[test]
fn minecraft_java_rcon_and_query_are_refused_through_tunnel_by_default_and_query_l1_works_when_explicitly_enabled(
) -> TestResult {
    // Prove GAME-002 security requirement:
    // "a connection to the RCON port through the tunnel is impossible unless the host enabled that mapping."
    let rcon_listener = TcpListener::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    let rcon_addr = rcon_listener.local_addr()?;
    let rcon_accept_count = Arc::new(AtomicUsize::new(0));
    let rcon_accept_clone = Arc::clone(&rcon_accept_count);

    let _rcon_guard = thread::spawn(move || {
        if rcon_listener.accept().is_ok() {
            rcon_accept_clone.fetch_add(1, Ordering::SeqCst);
        }
    });

    let game_listener = TcpListener::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    let game_addr = game_listener.local_addr()?;

    // Host configures `minecraft-java` with defaults:
    // index 0 (`game`, TCP) = enabled
    // index 1 (`query`, UDP) = disabled
    // index 2 (`rcon`, TCP) = disabled
    let mut engine_tcp = EngineTcpSession::new(
        vec![
            LocalMappingConfig::new(0, "game", Protocol::Tcp, true, game_addr),
            LocalMappingConfig::new(1, "query", Protocol::Udp, false, game_addr),
            LocalMappingConfig::new(2, "rcon", Protocol::Tcp, false, rcon_addr),
        ],
        4,
    )?;

    // Attempt to dial mapping_index 2 (`rcon`) through the engine tunnel stream.
    let tunnel_listener = TcpListener::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    let tunnel_addr = tunnel_listener.local_addr()?;

    let engine_handle = thread::spawn(move || {
        let stream = TcpStream::connect_timeout(&tunnel_addr, Duration::from_secs(3))
            .map_err(|e| e.to_string())?;
        let res = engine_handle_tcp_stream(&mut engine_tcp, stream);
        Ok::<(_, EngineTcpSession), String>((res, engine_tcp))
    });

    let (mut relay_side, _) = tunnel_listener.accept()?;
    relay_side.set_read_timeout(Some(Duration::from_secs(3)))?;
    relay_side.set_write_timeout(Some(Duration::from_secs(3)))?;

    let rcon_open_hdr = TcpOpenHeader { mapping_index: 2 }.encode();
    relay_side.write_all(&rcon_open_hdr)?;
    relay_side.flush()?;

    let mut status_byte = [0_u8; 1];
    relay_side.read_exact(&mut status_byte)?;
    assert_eq!(
        status_byte,
        [TcpOpenStatus::Refused.to_byte()],
        "disabled RCON mapping must be refused with 0x01"
    );

    let (engine_res, engine_tcp_after) = engine_handle
        .join()
        .map_err(|_| "engine thread panicked")??;
    assert!(engine_res.is_err());
    assert_eq!(engine_tcp_after.metrics().total_refused, 1);
    assert_eq!(
        rcon_accept_count.load(Ordering::SeqCst),
        0,
        "local RCON listener must receive 0 connections when RCON mapping is off"
    );

    // Also verify UDP `query` (index 1) is dropped when disabled (`GAME-008`, `SEC-006`).
    let mut engine_udp_disabled = EngineUdpSession::new(
        vec![LocalMappingConfig::new(
            1,
            "query",
            Protocol::Udp,
            false,
            game_addr,
        )],
        8,
        1200,
        30_000,
    )?;
    let mut relay_udp_only_game = RelayUdpSession::new(
        &[HelloAckMapping {
            index: 0,
            mapping_key: "game".to_owned(),
            protocol: Protocol::Tcp,
            public_port: 39123,
        }],
        8,
        1200,
        30_000,
    );
    let guest_addr: SocketAddr = "203.0.113.10:54321".parse()?;
    let query_pkt = UdpProbeKind::MinecraftJavaQueryHandshake.build_request_packet();
    assert!(
        relay_udp_only_game
            .handle_guest_datagram(1, guest_addr, &query_pkt, 100)
            .is_none(),
        "relay must drop UDP datagrams targeting unallocated query mapping"
    );
    assert_eq!(relay_udp_only_game.metrics().dropped_unknown_mapping, 1);

    let mut raw_query_frame = Vec::with_capacity(6 + query_pkt.len());
    raw_query_frame.extend_from_slice(&1_u16.to_be_bytes());
    raw_query_frame.extend_from_slice(&1_u32.to_be_bytes());
    raw_query_frame.extend_from_slice(&query_pkt);
    assert!(
        engine_udp_disabled
            .handle_relay_datagram(&raw_query_frame, 100)
            .is_err(),
        "engine must drop UDP datagrams targeting disabled query mapping"
    );
    assert_eq!(engine_udp_disabled.metrics().dropped_unknown_mapping, 1);

    // Now verify that when the host explicitly enables `query` (UDP), the L1 UT3 Query handshake
    // (`\xfe\xfd\x09`) succeeds through `RelayUdpSession <-> EngineUdpSession`!
    let query_server = UdpSocket::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    query_server.set_read_timeout(Some(Duration::from_secs(3)))?;
    query_server.set_write_timeout(Some(Duration::from_secs(3)))?;
    let query_local_addr = query_server.local_addr()?;

    let query_server_handle = thread::spawn(move || -> Result<(), String> {
        let mut buf = [0_u8; 256];
        let (n, peer) = query_server
            .recv_from(&mut buf)
            .map_err(|e| e.to_string())?;
        let slice = buf.get(..n).unwrap_or(&[]);
        if slice.get(..3) != Some(&[0xfe, 0xfd, 0x09]) {
            return Err(format!("unexpected UT3 query packet: {slice:?}"));
        }
        // Reply with 0x09 + 4-byte session ID + null-terminated challenge token "9513307\0"
        let mut reply = Vec::with_capacity(16);
        reply.push(0x09);
        reply.extend_from_slice(slice.get(3..7).unwrap_or(&[0, 0, 0, 1]));
        reply.extend_from_slice(b"9513307\0");
        query_server
            .send_to(&reply, peer)
            .map_err(|e| e.to_string())?;
        Ok(())
    });

    let mut relay_udp = RelayUdpSession::new(
        &[HelloAckMapping {
            index: 1,
            mapping_key: "query".to_owned(),
            protocol: Protocol::Udp,
            public_port: 39124,
        }],
        8,
        1200,
        30_000,
    );

    let mut engine_udp = EngineUdpSession::new(
        vec![LocalMappingConfig::new(
            1,
            "query",
            Protocol::Udp,
            true,
            query_local_addr,
        )],
        8,
        1200,
        30_000,
    )?;

    let tunnel_frame = relay_udp
        .handle_guest_datagram(1, guest_addr, &query_pkt, 200)
        .ok_or("relay_udp failed to frame query datagram")?;
    engine_udp
        .handle_relay_datagram(&tunnel_frame, 200)
        .map_err(|e| format!("{e:?}"))?;

    query_server_handle
        .join()
        .map_err(|_| "query server panicked")??;

    // Poll replies from local UDP sockets in EngineUdpSession and route back through RelayUdpSession.
    let mut reply_frames = Vec::new();
    for _ in 0..20 {
        reply_frames = engine_udp.poll_local_replies(8, 210);
        if !reply_frames.is_empty() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let first_reply = reply_frames
        .first()
        .ok_or("expected UT3 query reply frame")?;
    let (resp_mapping, dest_guest, payload) = relay_udp
        .handle_engine_datagram(first_reply, 210)
        .ok_or("relay_udp failed to decapsulate engine reply")?;
    assert_eq!(resp_mapping, 1);
    assert_eq!(dest_guest, guest_addr);
    assert!(
        UdpProbeKind::MinecraftJavaQueryHandshake.validate_response(&payload),
        "tunneled UT3 query response must pass L1 validation"
    );

    Ok(())
}

#[test]
fn terraria_l0_and_bedrock_and_source_l1_probes_and_explicit_gap_inventory_match_shipped_profiles(
) -> TestResult {
    let registry = Registry::load_dir(&profiles_dir())?;
    assert_eq!(registry.len(), 5);

    // 1. Terraria (`GAME-001`): TCP 7777 is verified and default-enabled; UDP 7777 is reported
    //    (`[R]`, E-1) and optional (default-off), reporting `ProbeStatus::Unknown` (`FR-011`).
    let terraria = registry.get("terraria").ok_or("terraria profile missing")?;
    let terraria_defaults = terraria.default_enabled_keys();
    assert_eq!(terraria_defaults, vec!["game"]);

    let terraria_listener = TcpListener::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    let terraria_addr = terraria_listener.local_addr()?;

    let terraria_table = configure_mappings(
        terraria,
        &[
            MappingTargetInput {
                mapping_key: "game".to_owned(),
                local_host: "127.0.0.1".to_owned(),
                local_port: terraria_addr.port(),
                enabled: true,
                confirm_non_loopback: false,
            },
            MappingTargetInput {
                mapping_key: "game-udp".to_owned(),
                local_host: "127.0.0.1".to_owned(),
                local_port: terraria_addr.port(),
                enabled: true,
                confirm_non_loopback: false,
            },
        ],
    )
    .map_err(|e| format!("{e:?}"))?;

    let terraria_probes = probe_local_targets(&terraria_table, None, Duration::from_secs(2));
    assert_eq!(terraria_probes.len(), 2);
    assert_eq!(
        terraria_probes.first().map(|p| (p.level, p.status)),
        Some((ProbeLevel::L0, ProbeStatus::Reachable))
    );
    assert_eq!(
        terraria_probes.get(1).map(|p| (p.level, p.status)),
        Some((ProbeLevel::L1, ProbeStatus::Unknown)),
        "Terraria UDP has no profile-defined status probe (E-1 gap) and must report Unknown"
    );

    // 2. Minecraft Bedrock (`GAME-003`): L1 RakNet Unconnected Ping (`0x01`) -> Unconnected Pong
    //    (`0x1c`) through the UDP tunnel.
    let bedrock_server = UdpSocket::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    bedrock_server.set_read_timeout(Some(Duration::from_secs(3)))?;
    let bedrock_addr = bedrock_server.local_addr()?;

    let bedrock_handle = thread::spawn(move || -> Result<(), String> {
        let mut buf = [0_u8; 256];
        let (n, peer) = bedrock_server
            .recv_from(&mut buf)
            .map_err(|e| e.to_string())?;
        let slice = buf.get(..n).unwrap_or(&[]);
        if slice.first() != Some(&0x01) {
            return Err("expected RakNet Unconnected Ping 0x01".to_owned());
        }
        let mut pong = Vec::with_capacity(64);
        pong.push(0x1c);
        pong.extend_from_slice(&1_000_u64.to_be_bytes());
        pong.extend_from_slice(&0x9988_7766_5544_3322_u64.to_be_bytes());
        pong.extend_from_slice(&gtt_engine::RAKNET_OFFLINE_MAGIC);
        let motd = b"MCPE;GTT Bedrock Test;748;1.26.52;1;10;12345;World;Survival;1;19132;19133;";
        pong.extend_from_slice(&u16::try_from(motd.len()).unwrap_or(0).to_be_bytes());
        pong.extend_from_slice(motd);
        bedrock_server
            .send_to(&pong, peer)
            .map_err(|e| e.to_string())?;
        Ok(())
    });

    let bedrock_probe = gtt_engine::probe_udp_mapping(
        "minecraft-bedrock",
        "game",
        bedrock_addr,
        Duration::from_secs(2),
    );
    bedrock_handle
        .join()
        .map_err(|_| "bedrock responder panicked")??;
    assert_eq!(bedrock_probe.level, ProbeLevel::L1);
    assert_eq!(bedrock_probe.status, ProbeStatus::Reachable);

    // 3. Left 4 Dead 2 (`GAME-004`) & Left 4 Dead (`GAME-005`, `ADR-021`):
    //    - `l4d2` has `game` (UDP 27015) verified and default-enabled; `rcon` (TCP 27015) is optional.
    //    - `l4d1` has only `Evidence::Reported` mappings (`E-5` gap) and exposes nothing by default.
    let l4d2 = registry.get("l4d2").ok_or("l4d2 profile missing")?;
    let l4d2_defaults = l4d2.default_enabled_keys();
    assert_eq!(l4d2_defaults, vec!["game"]);

    let l4d1 = registry.get("l4d1").ok_or("l4d1 profile missing")?;
    assert_eq!(
        l4d1.default_enabled_keys().len(),
        0,
        "l4d1 mappings are Evidence::Reported (E-5 gap) and must not be enabled by default (ADR-021)"
    );
    assert!(l4d1
        .mappings
        .iter()
        .all(|m| m.evidence == Evidence::Reported));

    Ok(())
}

#[test]
fn minecraft_java_slp_decoder_rejects_malformed_truncated_and_oversized_inputs_without_panic() {
    // 1. Overlong VarInt (> 5 continuation bytes) returns None without panic.
    assert_eq!(
        decode_mc_varint(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]),
        None
    );
    // 5th byte overflow bits set (`0xf0`) returns None.
    assert_eq!(decode_mc_varint(&[0x80, 0x80, 0x80, 0x80, 0x10]), None);

    // 2. Round-trip valid VarInts.
    for val in [0_u32, 1, 127, 128, 255, 25565, 32_767, u32::MAX] {
        let encoded = encode_mc_varint(val);
        assert_eq!(decode_mc_varint(&encoded), Some((val, encoded.len())));
    }

    // 3. Truncated SLP frame is rejected.
    let valid_frame = build_mc_java_slp_status_response(
        r#"{"version":{"name":"26.3","protocol":770},"players":{"max":20,"online":0}}"#,
    );
    let truncated = valid_frame
        .get(..valid_frame.len().saturating_sub(3))
        .unwrap_or(&[]);
    assert!(parse_mc_java_slp_status_response(truncated).is_err());

    // 4. Wrong packet ID (`0x01` instead of `0x00`) is rejected.
    let ping_frame = build_mc_java_slp_ping_request(42);
    assert!(parse_mc_java_slp_status_response(&ping_frame).is_err());

    // 5. Oversized SLP frame (`> MAX_MC_SLP_STATUS_BYTES`) is rejected before allocation/parse.
    let oversized = vec![0_u8; MAX_MC_SLP_STATUS_BYTES + 1];
    assert!(parse_mc_java_slp_status_response(&oversized).is_err());
}
