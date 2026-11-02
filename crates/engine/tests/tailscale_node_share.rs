//! Integration tests for Free-tier Tailscale Tailnet Node Share hosting of Minecraft: Java Edition
//! (`ADR-022`, `context.md` §§1, 7, `contracts.md` §§12, 13, `FR-005`, `GAME-002`, `GAME-008`,
//! `SEC-006`, `SRV-005`, `SRV-006`, `SRV-007`, `SRV-011`).

use std::error::Error;
use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use gtt_engine::{
    build_mc_java_slp_status_response, decode_mc_varint, probe_minecraft_java_slp,
    AcquisitionRecord, ArtifactSource, DataPaths, Digest, EngineEvent, EngineLifecycle,
    ManagedServerState, ManagedServerSupervisor, MappingTargetInput, ProbeLevel, ProbeStatus,
    ProvisionableSet, SessionConnectivityMode, StaticTailscaleDetector, TailscaleBindMode,
    TailscaleError, CONFIRMED_PRESENCE_LABEL, OBSERVED_TRAFFIC_LABEL, TAILSCALE_ADMIN_MACHINES_URL,
};
use gtt_profiles::Registry;
use gtt_protocol::EngineSessionState;

type TestResult = Result<(), Box<dyn Error>>;
type MockServerHandle = thread::JoinHandle<()>;

const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

fn profiles_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|root| root.join("profiles"))
        .unwrap_or_else(|| PathBuf::from("profiles"))
}

fn minecraft_java_verified_set() -> ProvisionableSet {
    ProvisionableSet::from_verified(vec![AcquisitionRecord {
        profile_id: "minecraft-java".to_owned(),
        experiment: "E-15".to_owned(),
        source: "https://piston-data.mojang.com/v1/objects/server.jar".to_owned(),
        verification: "SHA-256 + Adoptium JRE 25".to_owned(),
    }])
}

fn spawn_mock_minecraft_java_slp_server(
    version_name: &'static str,
    protocol_version: u32,
) -> Result<(SocketAddr, Arc<AtomicBool>, MockServerHandle), Box<dyn Error>> {
    let listener = TcpListener::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    let addr = listener.local_addr()?;
    let _ = listener.set_nonblocking(true);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop);

    let handle = thread::spawn(move || {
        while !stop_clone.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

                    // Read initial bytes. An L0 connect-only probe closes immediately (0 bytes).
                    let mut buf = [0_u8; 512];
                    let n = match stream.read(&mut buf) {
                        Ok(0) => continue,
                        Ok(n) => n,
                        Err(_) => continue,
                    };

                    let received = buf.get(..n).unwrap_or(&[]);
                    let Some((handshake_len, hdr_bytes)) = decode_mc_varint(received) else {
                        continue;
                    };
                    let handshake_end =
                        hdr_bytes.saturating_add(usize::try_from(handshake_len).unwrap_or(0));

                    let mut trailing = received.get(handshake_end..).unwrap_or(&[]).to_vec();
                    while trailing.len() < 2 {
                        let mut more = [0_u8; 64];
                        let Ok(m) = stream.read(&mut more) else {
                            break;
                        };
                        if m == 0 {
                            break;
                        }
                        trailing.extend_from_slice(more.get(..m).unwrap_or(&[]));
                    }

                    let status_json = serde_json::json!({
                        "version": {
                            "name": version_name,
                            "protocol": protocol_version
                        },
                        "players": {
                            "max": 20,
                            "online": 1,
                            "sample": []
                        },
                        "description": {
                            "text": "Minecraft Java via Tailscale Node Share"
                        },
                        "enforcesSecureChat": true
                    })
                    .to_string();

                    let response_frame = build_mc_java_slp_status_response(&status_json);
                    if stream.write_all(&response_frame).is_err() {
                        continue;
                    }
                    let _ = stream.flush();

                    // Read 10-byte Ping request `[0x09, 0x01, ..8B payload..]` and echo as Pong.
                    let mut ping_buf = trailing.get(2..).unwrap_or(&[]).to_vec();
                    while ping_buf.len() < 10 {
                        let mut more = [0_u8; 32];
                        let Ok(m) = stream.read(&mut more) else {
                            break;
                        };
                        if m == 0 {
                            break;
                        }
                        ping_buf.extend_from_slice(more.get(..m).unwrap_or(&[]));
                    }
                    if let Some(ping_frame) = ping_buf.get(..10) {
                        let _ = stream.write_all(ping_frame);
                        let _ = stream.flush();
                    }
                    let _ = stream.shutdown(Shutdown::Both);
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });

    Ok((addr, stop, handle))
}

#[test]
fn minecraft_java_tailscale_node_share_forwards_slp_and_maintains_five_status_layers() -> TestResult
{
    let registry = Registry::load_dir(&profiles_dir())?;
    let profile = registry
        .get("minecraft-java")
        .ok_or("missing minecraft-java profile")?;
    let (mc_local_addr, stop_mc, mc_handle) = spawn_mock_minecraft_java_slp_server("1.21.4", 769)?;

    // Configure Minecraft: Java Edition on 127.0.0.1:<mc_local_addr.port()>.
    // Optional mappings (rcon, query) are omitted so they stay default-off (`GAME-002`, `GAME-008`).
    let inputs = vec![MappingTargetInput {
        mapping_key: "game".to_owned(),
        local_host: "127.0.0.1".to_owned(),
        local_port: mc_local_addr.port(),
        enabled: true,
        confirm_non_loopback: false,
    }];

    let mut lifecycle = EngineLifecycle::configure(profile, &inputs, "0.1.0-test")
        .map_err(|e| format!("configure lifecycle failed: {e:?}"))?;
    assert_eq!(
        lifecycle.connectivity_mode(),
        SessionConnectivityMode::GttRelay
    );

    // Attach a managed server supervisor for Minecraft: Java Edition (`T-076`, `SRV-005`).
    let temp_dir =
        std::env::temp_dir().join(format!("gtt-tailscale-mc-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir)?;
    let paths = DataPaths::new(&temp_dir, "gtt-app")?;
    let supervisor = ManagedServerSupervisor::new(paths.clone(), minecraft_java_verified_set());
    lifecycle.attach_server_supervisor(supervisor);

    let staged_jar = temp_dir.join("staged-server.jar");
    fs::write(&staged_jar, b"abc")?;
    let artifact = ArtifactSource {
        path: staged_jar,
        expected: Digest::parse_hex(ABC_SHA256)?,
    };
    let (_acq, gen_cfg) = lifecycle.provision_server(profile, &artifact, "1.21.4", 8)?;

    // Verify `SRV-004`, `SRV-005` & `GAME-002`: server port is 25565 and RCON/Query stay off.
    assert_eq!(gen_cfg.server_port, 25565);
    assert!(!gen_cfg.rcon_enabled);
    assert!(!gen_cfg.query_enabled);
    let props_path = paths.resolve(&["servers", "minecraft-java", "server.properties"])?;
    let props_text = fs::read_to_string(&props_path)?;
    assert!(props_text.contains("server-port=25565\n"));
    assert!(props_text.contains("enable-rcon=false\n"));
    assert!(props_text.contains("enable-query=false\n"));

    // Start the Tailscale Node Share session using a simulated `tailscale status --json` output.
    let ts_json = r#"{
        "BackendState": "Running",
        "Self": {
            "HostName": "host-desktop",
            "DNSName": "host-desktop.tail9988.ts.net.",
            "Online": true,
            "TailscaleIPs": [
                "100.101.102.103",
                "fd7a:115c:a1e0::1"
            ]
        }
    }"#;
    let detector = StaticTailscaleDetector::from_json(ts_json);

    let endpoints = lifecycle.start_via_tailscale_with_bind_mode(
        &detector,
        TailscaleBindMode::LoopbackForTest {
            bind_ip: Ipv4Addr::LOCALHOST,
            bind_port: 0,
        },
    )?;

    // Only the `game` (TCP) mapping is exposed; RCON and Query are not exposed (`GAME-002`, `GAME-008`).
    assert_eq!(endpoints.len(), 1);
    let game_ep = endpoints
        .first()
        .ok_or("expected 1 Tailscale share endpoint")?;
    assert_eq!(game_ep.mapping_key, "game");
    assert_eq!(game_ep.tailscale_ipv4, Ipv4Addr::new(100, 101, 102, 103));
    assert_eq!(
        game_ep.dns_name.as_deref(),
        Some("host-desktop.tail9988.ts.net")
    );
    assert_eq!(
        game_ep.display_tailscale_ip_address(),
        format!("100.101.102.103:{}", game_ep.public_port)
    );
    assert_eq!(
        game_ep.display_magic_dns_address().as_deref(),
        Some(format!("host-desktop.tail9988.ts.net:{}", game_ep.public_port).as_str())
    );

    assert_eq!(
        lifecycle.connectivity_mode(),
        SessionConnectivityMode::TailscaleNodeShare
    );
    assert_eq!(lifecycle.state(), EngineSessionState::Active);
    assert_eq!(
        lifecycle.tailscale_status().map(|s| s.share_admin_url),
        Some(TAILSCALE_ADMIN_MACHINES_URL)
    );

    // Connect as a guest through the Tailscale Node Share listener and run a full Minecraft Java
    // Server List Ping (SLP) + Ping/Pong round trip (`probe_minecraft_java_slp`).
    let slp_report = probe_minecraft_java_slp(
        game_ep.bound_listener_addr,
        769,
        0x0102_0304_0506_0708,
        Duration::from_secs(2),
    )?;
    assert_eq!(slp_report.status.version.name, "1.21.4");
    assert_eq!(slp_report.status.version.protocol, 769);
    assert_eq!(slp_report.status.players.max, 20);
    assert_eq!(slp_report.status.players.online, 1);
    assert!(slp_report.pong_matched);

    // Poll local probes and Tailscale players-seen and simulate managed server readiness + MSMP join.
    let probes = lifecycle.probe_local(Some("game"), Duration::from_secs(2));
    assert_eq!(probes.len(), 1);
    let first_probe = probes.first().ok_or("expected 1 local probe result")?;
    assert_eq!(first_probe.level, ProbeLevel::L0);
    assert_eq!(first_probe.status, ProbeStatus::Reachable);

    let players_seen = lifecycle.poll_tailscale_players_seen();
    assert_eq!(players_seen.distinct_sources, 1);
    assert_eq!(players_seen.label, OBSERVED_TRAFFIC_LABEL);

    lifecycle.ingest_managed_server_line(
        "[16:40:00] [Server thread/INFO]: Done (1.420s)! For help, type \"help\"",
    );
    lifecycle.ingest_managed_server_line(
        r#"{"jsonrpc":"2.0","method":"minecraft:notification/players/joined","params":{"player":{"id":"player-uuid-1","name":"Alex"}}}"#,
    );

    // Verify all 5 status layers in `get_status()` are populated and independent (`FR-010`, `UX-002`, `SRV-006`, `SRV-011`).
    let snapshot = lifecycle.get_status();
    assert_eq!(snapshot.tunnel_state, EngineSessionState::Active);
    assert_eq!(snapshot.public_endpoints.len(), 1);
    assert_eq!(
        snapshot
            .public_endpoints
            .first()
            .map(|e| e.relay_host.as_str()),
        Some("100.101.102.103")
    );
    assert_eq!(snapshot.local_probes.len(), 1);
    assert_eq!(
        snapshot.local_probes.first().map(|p| p.status),
        Some(ProbeStatus::Reachable)
    );
    assert_eq!(snapshot.players_seen.distinct_sources, 1);
    assert_eq!(snapshot.players_seen.label, OBSERVED_TRAFFIC_LABEL);
    assert_eq!(
        snapshot.managed_server.state,
        ManagedServerState::Configuring
    );
    assert_eq!(snapshot.confirmed_presence.confirmed_players, 1);
    assert_eq!(snapshot.confirmed_presence.label, CONFIRMED_PRESENCE_LABEL);

    // Stop the session and verify the Tailscale forwarder and session transition cleanly to Stopped.
    let _ = lifecycle.stop("user_stop")?;
    assert_eq!(lifecycle.state(), EngineSessionState::Stopped);
    assert!(lifecycle.tailscale_share_endpoints().is_empty());

    stop_mc.store(true, Ordering::SeqCst);
    let _ = TcpStream::connect_timeout(&mc_local_addr, Duration::from_millis(50));
    let _ = mc_handle.join();
    let _ = fs::remove_dir_all(&temp_dir);
    Ok(())
}

#[test]
fn tailscale_node_share_surfaces_actionable_errors_when_not_ready() -> TestResult {
    let registry = Registry::load_dir(&profiles_dir())?;
    let profile = registry
        .get("minecraft-java")
        .ok_or("missing minecraft-java profile")?;
    let inputs = vec![MappingTargetInput {
        mapping_key: "game".to_owned(),
        local_host: "127.0.0.1".to_owned(),
        local_port: 25565,
        enabled: true,
        confirm_non_loopback: false,
    }];

    let mut lifecycle = EngineLifecycle::configure(profile, &inputs, "0.1.0-test")
        .map_err(|e| format!("configure lifecycle failed: {e:?}"))?;

    let needs_login_detector = StaticTailscaleDetector::from_json(
        r#"{"BackendState": "NeedsLogin", "Self": {"Online": false}}"#,
    );
    let res = lifecycle.start_via_tailscale(&needs_login_detector);
    assert_eq!(
        res,
        Err(TailscaleError::NotAuthenticated {
            state: "NeedsLogin".to_owned()
        })
    );
    if let Err(err) = res {
        assert_eq!(err.code(), "tailscale_not_authenticated");
    }
    assert_eq!(lifecycle.state(), EngineSessionState::Idle);

    let events = lifecycle.drain_events();
    assert!(events.iter().any(|ev| matches!(
        ev,
        EngineEvent::EngineError { code, .. } if code == "tailscale_not_authenticated"
    )));
    Ok(())
}
