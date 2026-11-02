//! Tests for local server reachability probes (`T-072`, `FR-011`, `contracts.md` §13) against
//! listening and stopped TCP and UDP servers.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, UdpSocket};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use gtt_engine::{
    configure_mappings, probe_local_targets, probe_tcp_connect, probe_udp_mapping,
    MappingTargetInput, ProbeLevel, ProbeStatus, RAKNET_OFFLINE_MAGIC,
};
use gtt_profiles::Registry;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn repo_profiles_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("profiles")
}

#[test]
fn tcp_connect_probe_reports_reachable_for_silent_listening_server_and_unreachable_when_stopped(
) -> TestResult {
    // Bind a real TCP listener that accepts connections and sends zero bytes (`FR-011`,
    // `research.md` §5 RQ-13.2c: Minecraft Java sends nothing until the client speaks first).
    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
    let addr = listener.local_addr()?;

    let accept_handle = thread::spawn(move || {
        if let Ok((_stream, _peer)) = listener.accept() {
            // Hold stream briefly without sending any bytes, then drop listener.
        }
    });

    let live_result = probe_tcp_connect("game", addr, Duration::from_millis(300));
    assert_eq!(live_result.mapping_key, "game");
    assert_eq!(live_result.level, ProbeLevel::L0);
    assert_eq!(live_result.status, ProbeStatus::Reachable);
    assert!(live_result.ok);

    let _ = accept_handle.join();

    // Once the listener is dropped/stopped, the same address must report Unreachable.
    let stopped_result = probe_tcp_connect("game", addr, Duration::from_millis(200));
    assert_eq!(stopped_result.level, ProbeLevel::L0);
    assert_eq!(stopped_result.status, ProbeStatus::Unreachable);
    assert!(!stopped_result.ok);

    Ok(())
}

#[test]
fn udp_probes_report_reachable_only_on_valid_response_and_never_without_response() -> TestResult {
    // 1. Minecraft Bedrock (`RakNetUnconnectedPing`: 0x01 -> 0x1c + magic at 17..33).
    let bedrock_sock = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
    bedrock_sock.set_read_timeout(Some(Duration::from_secs(2)))?;
    let bedrock_addr = bedrock_sock.local_addr()?;

    let bedrock_server = thread::spawn(move || -> Result<(), std::io::Error> {
        let mut buf = [0_u8; 256];
        let (n, peer) = bedrock_sock.recv_from(&mut buf)?;
        assert_eq!(n, 33);
        assert_eq!(buf.first(), Some(&0x01));
        // Build valid RakNet Unconnected Pong (0x1c + 8B time + 8B server GUID + 16B magic + server string).
        let mut pong = Vec::with_capacity(48);
        pong.push(0x1c);
        pong.extend_from_slice(&1_000_u64.to_be_bytes());
        pong.extend_from_slice(&0x9988_7766_5544_3322_u64.to_be_bytes());
        pong.extend_from_slice(&RAKNET_OFFLINE_MAGIC);
        pong.extend_from_slice(b"\x00\x0bMCPE;Dedicated;");
        bedrock_sock.send_to(&pong, peer)?;
        Ok(())
    });

    let bedrock_live = probe_udp_mapping(
        "minecraft-bedrock",
        "game",
        bedrock_addr,
        Duration::from_millis(400),
    );
    assert_eq!(bedrock_live.level, ProbeLevel::L1);
    assert_eq!(bedrock_live.status, ProbeStatus::Reachable);
    assert!(bedrock_live.ok);
    let _ = bedrock_server.join();

    // 2. When the Bedrock UDP server is stopped, or when a UDP socket is bound and listening
    //    silently without sending a response (`FR-011`), UDP probe MUST report Unreachable (never
    //    Reachable without a response).
    let silent_udp = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
    let silent_addr = silent_udp.local_addr()?;
    let silent_probe = probe_udp_mapping(
        "minecraft-bedrock",
        "game",
        silent_addr,
        Duration::from_millis(120),
    );
    assert_eq!(silent_probe.status, ProbeStatus::Unreachable);
    assert!(!silent_probe.ok);
    drop(silent_udp);

    // 3. Left 4 Dead 2 (`SourceA2sInfo`: `\xff\xff\xff\xffTSource Engine Query\x00` -> `\xff\xff\xff\xffI...`).
    let l4d2_sock = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
    l4d2_sock.set_read_timeout(Some(Duration::from_secs(2)))?;
    let l4d2_addr = l4d2_sock.local_addr()?;

    let l4d2_server = thread::spawn(move || -> Result<(), std::io::Error> {
        let mut buf = [0_u8; 256];
        let (_n, peer) = l4d2_sock.recv_from(&mut buf)?;
        l4d2_sock.send_to(b"\xff\xff\xff\xffI\x11Left4Dead2Server\x00", peer)?;
        Ok(())
    });

    let l4d2_live = probe_udp_mapping("l4d2", "game", l4d2_addr, Duration::from_millis(400));
    assert_eq!(l4d2_live.status, ProbeStatus::Reachable);
    assert!(l4d2_live.ok);
    let _ = l4d2_server.join();

    // 4. Malformed UDP response bytes must be rejected as Unreachable (`FR-011`).
    let bad_udp = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
    bad_udp.set_read_timeout(Some(Duration::from_secs(2)))?;
    let bad_addr = bad_udp.local_addr()?;
    let bad_server = thread::spawn(move || -> Result<(), std::io::Error> {
        let mut buf = [0_u8; 256];
        let (_n, peer) = bad_udp.recv_from(&mut buf)?;
        bad_udp.send_to(b"garbage-not-a2s", peer)?;
        Ok(())
    });

    let bad_res = probe_udp_mapping("l4d2", "game", bad_addr, Duration::from_millis(400));
    assert_eq!(bad_res.status, ProbeStatus::Unreachable);
    assert!(!bad_res.ok);
    let _ = bad_server.join();

    Ok(())
}

#[test]
fn udp_mappings_without_profile_defined_probe_report_unknown() -> TestResult {
    let registry = Registry::load_dir(&repo_profiles_dir())?;
    let terraria = registry.get("terraria").ok_or("missing terraria profile")?;

    let tcp_listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
    let tcp_port = tcp_listener.local_addr()?.port();

    let inputs = vec![
        MappingTargetInput {
            mapping_key: "game".to_owned(),
            local_host: "127.0.0.1".to_owned(),
            local_port: tcp_port,
            enabled: true,
            confirm_non_loopback: false,
        },
        MappingTargetInput {
            mapping_key: "game-udp".to_owned(),
            local_host: "127.0.0.1".to_owned(),
            local_port: 7777,
            enabled: true,
            confirm_non_loopback: false,
        },
    ];

    let table =
        configure_mappings(terraria, &inputs).map_err(|e| format!("unexpected failure: {e:?}"))?;
    let results = probe_local_targets(&table, None, Duration::from_millis(250));
    assert_eq!(results.len(), 2);

    let tcp_res = results
        .iter()
        .find(|r| r.mapping_key == "game")
        .ok_or("missing game probe result")?;
    assert_eq!(tcp_res.status, ProbeStatus::Reachable);
    assert!(tcp_res.ok);

    let udp_res = results
        .iter()
        .find(|r| r.mapping_key == "game-udp")
        .ok_or("missing game-udp probe result")?;
    assert_eq!(udp_res.status, ProbeStatus::Unknown);
    assert!(!udp_res.ok);

    Ok(())
}
