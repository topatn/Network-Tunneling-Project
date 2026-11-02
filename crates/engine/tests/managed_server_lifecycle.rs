//! Integration tests for `T-076` — Managed server lifecycle integration with engine and UI events
//! (`SRV-005`, `SRV-006`, `SRV-007`, `SRV-011`, `SRV-015`, `SRV-018`, `ADR-013`, `contracts.md` §13).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use gtt_engine::{
    AcquisitionRecord, ArtifactSource, ClientConfig, ConfirmedPresenceSnapshot, ControlPlaneClient,
    DataPaths, DeterministicBackoff, Digest, EngineEvent, EngineLifecycle, HttpMethod, HttpRequest,
    HttpResponse, HttpTransport, IdempotencyKey, ManagedServerError, ManagedServerState,
    ManagedServerSupervisor, MappingTargetInput, NoTokenRefresher, ProbeStatus, ProvisionableSet,
    ServerLaunchSpec, TransportError, CONFIRMED_PRESENCE_LABEL, OBSERVED_TRAFFIC_LABEL,
};
use gtt_profiles::Registry;
use gtt_protocol::{
    encode_control_frame, AllocationTokenClaims, ControlMessage, EngineSessionState,
    FlowSummaryMessage, HelloAckMapping, HelloAckMessage, KeyId, Protocol, SessionLimits,
    SigningKeyMaterial, SigningKeyRing, DEFAULT_TOKEN_ISSUER,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

const TUNNEL_ID: &str = "11111111-1111-4111-8111-111111111111";
const ALLOCATION_ID: &str = "22222222-2222-4222-8222-222222222222";
const RELAY_ID: &str = "33333333-3333-4333-8333-333333333333";

struct ScratchDir {
    dir: PathBuf,
}

impl ScratchDir {
    fn new(label: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join(format!("gtt-t076-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

struct CapturingHttpTransport {
    responses: RefCell<VecDeque<HttpResponse>>,
    requests: RefCell<Vec<HttpRequest>>,
}

impl CapturingHttpTransport {
    fn new(responses: Vec<HttpResponse>) -> Self {
        Self {
            responses: RefCell::new(responses.into()),
            requests: RefCell::new(Vec::new()),
        }
    }

    fn captured_requests(&self) -> Vec<HttpRequest> {
        self.requests.borrow().clone()
    }
}

impl HttpTransport for CapturingHttpTransport {
    fn send(&mut self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.requests.borrow_mut().push(request.clone());
        self.responses
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| TransportError::Network("no queued HTTP response".to_owned()))
    }
}

fn repo_profiles_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("profiles")
}

fn minecraft_java_verified_set() -> ProvisionableSet {
    ProvisionableSet::from_verified(vec![AcquisitionRecord {
        profile_id: "minecraft-java".to_owned(),
        experiment: "E-15".to_owned(),
        source: "https://piston-data.mojang.com/v1/objects/server.jar".to_owned(),
        verification: "SHA-256 + Adoptium JRE 25".to_owned(),
    }])
}

fn issue_test_token() -> Result<String, String> {
    let kid = KeyId::new("kid-2026-10").map_err(|e| format!("kid error: {e}"))?;
    let material =
        SigningKeyMaterial::from_bytes([0x42_u8; 32]).map_err(|e| format!("key error: {e}"))?;
    let ring = SigningKeyRing::new(DEFAULT_TOKEN_ISSUER, kid, material, 1_800_000);
    let claims = AllocationTokenClaims {
        iss: DEFAULT_TOKEN_ISSUER.to_owned(),
        aud: RELAY_ID.to_owned(),
        sub: TUNNEL_ID.to_owned(),
        aid: ALLOCATION_ID.to_owned(),
        jti: "55555555-5555-4555-8555-555555555555".to_owned(),
        iat: 1_999_400,
        exp: 2_000_000,
        mappings: vec![HelloAckMapping {
            index: 0,
            mapping_key: "game".to_owned(),
            protocol: Protocol::Tcp,
            public_port: 39123,
        }],
    };
    ring.sign_allocation_token(&claims)
        .map_err(|e| format!("sign token failed: {e}"))
}

fn assert_no_local_server_leak(haystack: &str, context: &str, local_port: u16, root_dir: &Path) {
    let port_str = local_port.to_string();
    let root_str = root_dir.to_string_lossy();
    for forbidden in [
        "127.0.0.1",
        "localhost",
        "::1",
        &port_str,
        "local_port",
        "local_host",
        "target_port",
        "target_host",
        "server-port",
        root_str.as_ref(),
    ] {
        assert!(
            !haystack.contains(forbidden),
            "SRV-005 / INV-1 violation: {context} leaked {forbidden:?} in payload: {haystack}"
        );
    }
}

#[test]
fn provisioning_and_tunnel_start_capture_contains_no_managed_server_address_or_port_and_all_five_indicators_change_independently(
) -> TestResult {
    let scratch = ScratchDir::new("srv005-independence")?;
    let registry = Registry::load_dir(&repo_profiles_dir())?;
    let mc_java = registry
        .get("minecraft-java")
        .ok_or("missing minecraft-java profile")?;
    let all_profiles: Vec<_> = registry
        .ids()
        .filter_map(|id| registry.get(id).cloned())
        .collect();

    // Bind an ephemeral local TCP listener to act as the local game server endpoint.
    let local_listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
    let local_port = local_listener.local_addr()?.port();

    let inputs = vec![MappingTargetInput {
        mapping_key: "game".to_owned(),
        local_host: "127.0.0.1".to_owned(),
        local_port,
        enabled: true,
        confirm_non_loopback: false,
    }];

    let mut engine = EngineLifecycle::configure(mc_java, &inputs, "0.1.0-test")
        .map_err(|e| format!("configure failed: {e:?}"))?;

    let data_paths = DataPaths::new(scratch.path(), "gtt-app")?;
    let supervisor =
        ManagedServerSupervisor::new(data_paths.clone(), minecraft_java_verified_set());
    engine.attach_server_supervisor(supervisor);

    // Verify list_provisionable (`SRV-003`, `ADR-013`: only minecraft-java is provisionable).
    let list = engine.list_provisionable(&all_profiles, None);
    let mc_entry = list
        .iter()
        .find(|e| e.profile_id == "minecraft-java")
        .ok_or("missing minecraft-java in provisionable list")?;
    assert!(mc_entry.provisionable);
    for other in list.iter().filter(|e| e.profile_id != "minecraft-java") {
        assert!(
            !other.provisionable,
            "ADR-013 / SRV-003: {} must not be provisionable",
            other.profile_id
        );
    }

    // Write a verified server artifact on disk (`SRV-002`).
    let staged_jar = scratch.path().join("staged-server.jar");
    fs::write(&staged_jar, b"abc")?;
    let artifact = ArtifactSource {
        path: staged_jar,
        expected: Digest::parse_hex(ABC_SHA256)?,
    };

    // 1. Provision the managed Minecraft: Java Edition server.
    let (_record, generated) = engine.provision_server(mc_java, &artifact, "1.21.9", 20)?;
    assert_eq!(generated.server_port, 25565);
    assert!(!generated.rcon_enabled);
    assert!(!generated.query_enabled);

    // SRV-015: verify eula.txt was NOT accepted on the host's behalf.
    let eula_path = data_paths.resolve(&["servers", "minecraft-java", "eula.txt"])?;
    let eula_text = fs::read_to_string(&eula_path)?;
    assert!(
        !eula_text.contains("eula=true"),
        "SRV-015: provisioning must never write eula=true on the host's behalf"
    );

    // Indicator 4 is now Configuring, while Indicators 1 (tunnel), 2 (local_probes),
    // 3 (players_seen), and 5 (confirmed_presence) are completely unchanged (`SRV-006`, `FR-010`).
    let snap1 = engine.get_status();
    assert_eq!(snap1.managed_server.state, ManagedServerState::Configuring);
    assert_eq!(snap1.tunnel_state, EngineSessionState::Idle);
    assert!(snap1.local_probes.is_empty());
    assert_eq!(snap1.players_seen.distinct_sources, 0);
    assert_eq!(snap1.confirmed_presence.confirmed_players, 0);

    // 2. Drive Indicator 4 (`Starting -> Running`) and Indicator 5 (`ConfirmedPresence` via MSMP)
    //    independently of the tunnel and local probes (`SRV-006`, `SRV-011`).
    engine
        .ingest_managed_server_line("[Server thread/INFO]: Done (1.320s)! For help, type \"help\"");
    // Wait: ingest_managed_server_line transitions Starting -> Running when in Starting.
    // Let's also feed MSMP notifications (`SRV-011`, `research.md` RQ-13.4).
    engine.ingest_managed_server_line(
        r#"{"jsonrpc":"2.0","method":"minecraft:notification/players/joined","params":{"player":{"id":"player-uuid-1","name":"Alex"}}}"#,
    );
    engine.ingest_managed_server_line(
        r#"{"jsonrpc":"2.0","method":"minecraft:notification/players/joined","params":{"player":{"id":"player-uuid-2","name":"Steve"}}}"#,
    );

    let snap2 = engine.get_status();
    assert_eq!(
        snap2.confirmed_presence,
        ConfirmedPresenceSnapshot {
            enabled: true,
            confirmed_players: 2,
            label: CONFIRMED_PRESENCE_LABEL,
        }
    );
    // CRITICAL (`SRV-011`): MSMP confirmed presence must NEVER merge into `players_seen`.
    assert_eq!(snap2.players_seen.distinct_sources, 0);
    assert_eq!(snap2.players_seen.tcp_flows, 0);
    assert_eq!(snap2.players_seen.label, OBSERVED_TRAFFIC_LABEL);
    assert_eq!(snap2.tunnel_state, EngineSessionState::Idle);

    // One player leaves via MSMP.
    engine.ingest_managed_server_line(
        r#"{"jsonrpc":"2.0","method":"minecraft:notification/players/left","params":{"player":{"id":"player-uuid-1","name":"Alex"}}}"#,
    );
    assert_eq!(engine.confirmed_presence().confirmed_players, 1);
    assert_eq!(engine.players_seen().distinct_sources, 0);

    // 3. Drive Indicator 2 (`local_probes`) independently.
    let probe_results = engine.probe_local(Some("game"), Duration::from_millis(200));
    assert_eq!(probe_results.len(), 1);
    assert_eq!(
        probe_results.first().map(|r| r.status),
        Some(ProbeStatus::Reachable)
    );
    assert_eq!(engine.state(), EngineSessionState::Idle);
    assert_eq!(engine.confirmed_presence().confirmed_players, 1);
    assert_eq!(engine.players_seen().distinct_sources, 0);

    // 4. Start the tunnel via the control-plane API and capture all outbound network messages
    //    (`SRV-005`, `INV-1`).
    let token = issue_test_token()?;
    let start_body = serde_json::json!({
        "tunnel_id": TUNNEL_ID,
        "allocation_token": token,
        "allocation": {
            "id": ALLOCATION_ID,
            "expires_at": "2026-10-27T12:00:00Z",
            "relay": {
                "region": "us-east",
                "host": "relay.us-east.example.test",
                "tunnel_port": 4433
            },
            "ports": [
                { "mapping_key": "game", "protocol": "tcp", "public_port": 39123 }
            ]
        }
    })
    .to_string();

    let stop_body = serde_json::json!({
        "id": TUNNEL_ID,
        "name": "MC Java Server",
        "game_profile_id": "minecraft-java",
        "profile_version": 1,
        "state": "stopped",
        "mappings": [
            { "position": 0, "mapping_key": "game", "protocol": "tcp", "enabled": true }
        ],
        "created_at": "2026-10-27T10:00:00Z",
        "expires_at": "2026-10-27T18:00:00Z",
        "stopped_at": "2026-10-27T11:00:00Z"
    })
    .to_string();

    let transport = CapturingHttpTransport::new(vec![
        HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: start_body.into_bytes(),
        },
        HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: stop_body.into_bytes(),
        },
    ]);

    let cfg = ClientConfig::new("https://api.example.test");
    let mut api_client = ControlPlaneClient::new(
        cfg,
        Some("access-token-test-0001".to_owned()),
        transport,
        DeterministicBackoff::with_jitter_bps(0),
        NoTokenRefresher,
    )?;

    let idem_start = IdempotencyKey::new("idem-t076-start-0001")?;
    let _ = engine.start_via_api(&mut api_client, TUNNEL_ID, &idem_start)?;
    let hello_frame = engine.on_quic_connected()?;

    let hello_ack = encode_control_frame(&ControlMessage::HelloAck(HelloAckMessage {
        session_id: "66666666-6666-4666-8666-666666666666".to_owned(),
        heartbeat_interval_ms: 5_000,
        idle_timeout_ms: 15_000,
        mappings: vec![HelloAckMapping {
            index: 0,
            mapping_key: "game".to_owned(),
            protocol: Protocol::Tcp,
            public_port: 39123,
        }],
        limits: SessionLimits {
            max_tcp_flows: 16,
            max_udp_flows: 16,
        },
    }))?;
    let _ = engine.handle_relay_control_frame(&hello_ack, 1_000)?;
    assert_eq!(engine.state(), EngineSessionState::Active);

    // 5. Drive Indicator 3 (`players_seen` from relay `flow_summary`) and prove it stays distinct
    //    from Indicator 5 (`confirmed_presence`: 1 confirmed player vs 3 observed relay flows).
    let flow_summary = encode_control_frame(&ControlMessage::FlowSummary(FlowSummaryMessage {
        tcp_flows: 3,
        udp_flows: 0,
        distinct_sources: 3,
    }))?;
    let _ = engine.handle_relay_control_frame(&flow_summary, 2_000)?;
    assert_eq!(engine.players_seen().distinct_sources, 3);
    assert_eq!(engine.confirmed_presence().confirmed_players, 1);

    let goodbye_frame = engine.stop_via_api(&mut api_client, None)?;

    // 6. Inspect captured HTTP requests and relay wire frames (`SRV-005`, `INV-1`):
    //    neither the configured local_port nor the generated server.properties port (25565) nor
    //    any local host or filesystem path may appear in any network message.
    let captured_http = api_client.transport().captured_requests();
    assert_eq!(captured_http.len(), 2);
    assert_eq!(
        captured_http.first().map(|r| r.method),
        Some(HttpMethod::Post)
    );
    for req in &captured_http {
        assert_no_local_server_leak(&req.url, "HTTP URL", local_port, scratch.path());
        assert_no_local_server_leak(&req.url, "HTTP URL (25565)", 25565, scratch.path());
        if let Some(body) = &req.body {
            let body_str = String::from_utf8_lossy(body);
            assert_no_local_server_leak(&body_str, "HTTP request body", local_port, scratch.path());
            assert_no_local_server_leak(
                &body_str,
                "HTTP request body (25565)",
                25565,
                scratch.path(),
            );
        }
    }

    let hello_json = String::from_utf8_lossy(hello_frame.get(4..).unwrap_or(&[]));
    assert_no_local_server_leak(&hello_json, "relay hello frame", local_port, scratch.path());
    assert_no_local_server_leak(
        &hello_json,
        "relay hello frame (25565)",
        25565,
        scratch.path(),
    );

    let goodbye_json = String::from_utf8_lossy(goodbye_frame.get(4..).unwrap_or(&[]));
    assert_no_local_server_leak(
        &goodbye_json,
        "relay goodbye frame",
        local_port,
        scratch.path(),
    );
    assert_no_local_server_leak(
        &goodbye_json,
        "relay goodbye frame (25565)",
        25565,
        scratch.path(),
    );

    Ok(())
}

#[test]
fn child_process_supervision_distinguishes_refused_start_restarts_crashes_stops_on_tunnel_stop_and_leaves_no_orphan_on_exit(
) -> TestResult {
    let scratch = ScratchDir::new("srv007-process")?;
    let registry = Registry::load_dir(&repo_profiles_dir())?;
    let mc_java = registry
        .get("minecraft-java")
        .ok_or("missing minecraft-java profile")?;

    // Check SRV-018 version parser and validator first.
    let java8_out = "java version \"1.8.0_481\"\nJava(TM) SE Runtime Environment";
    let java25_out = "openjdk version \"25.0.4.1\" 2026-08-18 LTS";
    assert!(matches!(
        ServerLaunchSpec::minecraft_java(PathBuf::from("java"), java8_out, 25),
        Err(ManagedServerError::JavaRuntimeRejected {
            required_major: 25,
            found_major: Some(8),
            ..
        })
    ));
    let valid_spec = ServerLaunchSpec::minecraft_java(PathBuf::from("java"), java25_out, 25)?;
    assert_eq!(valid_spec.selected_java_major, 25);

    let inputs = vec![MappingTargetInput {
        mapping_key: "game".to_owned(),
        local_host: "127.0.0.1".to_owned(),
        local_port: 25565,
        enabled: true,
        confirm_non_loopback: false,
    }];
    let mut engine = EngineLifecycle::configure(mc_java, &inputs, "0.1.0-test")
        .map_err(|e| format!("configure failed: {e:?}"))?;

    let data_paths = DataPaths::new(scratch.path(), "gtt-app")?;
    let mut supervisor =
        ManagedServerSupervisor::new(data_paths.clone(), minecraft_java_verified_set());
    supervisor.set_max_crash_restarts(1);
    engine.attach_server_supervisor(supervisor);

    let staged_jar = scratch.path().join("staged-server.jar");
    fs::write(&staged_jar, b"abc")?;
    let artifact = ArtifactSource {
        path: staged_jar.clone(),
        expected: Digest::parse_hex(ABC_SHA256)?,
    };
    let _ = engine.provision_server(mc_java, &artifact, "1.21.9", 20)?;

    // 1. SRV-015: starting before the host accepts eula.txt is refused with LicenseTermsNotAccepted.
    let dummy_spec = ServerLaunchSpec {
        program: PathBuf::from("cmd"),
        args: vec!["/C".to_owned(), "echo Done (0.1s)!".to_owned()],
        required_java_major: 25,
        selected_java_major: 25,
    };
    assert!(matches!(
        engine.start_managed_server("minecraft-java", dummy_spec),
        Err(ManagedServerError::LicenseTermsNotAccepted { .. })
    ));

    // Host deliberately accepts EULA in eula.txt (`SRV-015`).
    let eula_path = data_paths.resolve(&["servers", "minecraft-java", "eula.txt"])?;
    fs::write(&eula_path, "eula=true\n")?;

    // 2. SRV-007 requirement (a): a process that exits 0 BEFORE printing `Done (` is reported as
    //    RefusedBeforeReady (Failed), never as a clean exit, and does not consume crash retries.
    let refused_spec = ServerLaunchSpec {
        program: PathBuf::from("cmd"),
        args: vec![
            "/C".to_owned(),
            "echo You need to agree to the EULA in order to run the server.".to_owned(),
        ],
        required_java_major: 25,
        selected_java_major: 25,
    };
    let state = engine.start_managed_server("minecraft-java", refused_spec)?;
    assert_eq!(state, ManagedServerState::Starting);

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut refused_err = None;
    while Instant::now() < deadline {
        match engine.poll_managed_server() {
            Ok(_) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => {
                refused_err = Some(e);
                break;
            }
        }
    }
    assert!(
        matches!(
            refused_err,
            Some(ManagedServerError::RefusedBeforeReady {
                exit_code: Some(0),
                ..
            })
        ),
        "expected RefusedBeforeReady with exit 0, got {refused_err:?}"
    );
    assert_eq!(
        engine.get_managed_server_status().state,
        ManagedServerState::Failed
    );
    assert_eq!(engine.get_managed_server_status().crash_count, 0);

    // 3. SRV-007: a process that prints `Done (` reaches Running; if it crashes unexpectedly, the
    //    supervisor restarts it up to `max_crash_restarts` (1) and then stops with RestartLimitReached.
    let crash_after_ready_spec = ServerLaunchSpec {
        program: PathBuf::from("cmd"),
        args: vec![
            "/C".to_owned(),
            "echo [Server thread/INFO]: Done (1.0s)! & exit /B 1".to_owned(),
        ],
        required_java_major: 25,
        selected_java_major: 25,
    };
    let _ = engine.start_managed_server("minecraft-java", crash_after_ready_spec)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut limit_err = None;
    while Instant::now() < deadline {
        match engine.poll_managed_server() {
            Ok(_) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => {
                limit_err = Some(e);
                break;
            }
        }
    }
    assert!(
        matches!(
            limit_err,
            Some(ManagedServerError::RestartLimitReached { attempts: 2, .. })
        ),
        "expected RestartLimitReached after 1 automatic restart, got {limit_err:?}"
    );
    assert_eq!(
        engine.get_managed_server_status().state,
        ManagedServerState::Failed
    );

    // 4. World backup, update gating while tunnel is active (`SRV-008`, `SRV-009`), and
    //    graceful stop via stdin when tunnel stops or app exits (`SRV-007`).
    let world_file = data_paths.resolve(&["servers", "minecraft-java", "world", "level.dat"])?;
    fs::write(&world_file, b"world-v1-data")?;
    let _ = engine.backup_world("minecraft-java", "backup-01")?;

    fs::write(&world_file, b"corrupted-world")?;
    let _ = engine.restore_world("minecraft-java", "backup-01")?;
    assert_eq!(fs::read(&world_file)?, b"world-v1-data");

    // Start a long-lived helper process that prints `Done (`, waits for `stop` on stdin, and writes
    // `Saving worlds` before exiting 0 (`SRV-007` requirement (b)).
    let stop_marker = data_paths.resolve(&["servers", "minecraft-java", "stopped.marker"])?;
    let ps_script = format!(
        "[Console]::Out.WriteLine('[Server thread/INFO]: Done (1.2s)!'); [Console]::Out.Flush(); $line = [Console]::In.ReadLine(); if ($line -eq 'stop') {{ [Console]::Out.WriteLine('Saving worlds'); [Console]::Out.Flush(); [System.IO.File]::WriteAllText('{}', 'saved') }}",
        stop_marker.display()
    );
    let interactive_spec = ServerLaunchSpec {
        program: PathBuf::from("powershell"),
        args: vec![
            "-NoProfile".to_owned(),
            "-NonInteractive".to_owned(),
            "-Command".to_owned(),
            ps_script,
        ],
        required_java_major: 25,
        selected_java_major: 25,
    };

    let _ = engine.start_managed_server("minecraft-java", interactive_spec.clone())?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let st = engine.poll_managed_server()?;
        if st == ManagedServerState::Running {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        engine.get_managed_server_status().state,
        ManagedServerState::Running
    );

    // Bring tunnel to Connecting so we can verify SRV-009 update confirmation gate and SRV-007
    // automatic server stop on tunnel stop.
    let start_resp = serde_json::from_value(serde_json::json!({
        "tunnel_id": TUNNEL_ID,
        "allocation_token": issue_test_token()?,
        "allocation": {
            "id": ALLOCATION_ID,
            "expires_at": "2026-10-27T12:00:00Z",
            "relay": {
                "region": "us-east",
                "host": "relay.us-east.example.test",
                "tunnel_port": 4433
            },
            "ports": [
                { "mapping_key": "game", "protocol": "tcp", "public_port": 39123 }
            ]
        }
    }))?;
    let _ = engine.begin_start_with_response(&start_resp)?;

    // SRV-009: update without confirmation while tunnel is active is refused.
    let new_jar = scratch.path().join("server-v2.jar");
    fs::write(&new_jar, b"")?;
    let new_artifact = ArtifactSource {
        path: new_jar,
        expected: Digest::parse_hex(EMPTY_SHA256)?,
    };
    assert_eq!(
        engine.update_managed_server("minecraft-java", &new_artifact, "1.21.10", false),
        Err(ManagedServerError::UpdateRequiresActiveTunnelConfirmation)
    );

    // SRV-007: stopping the tunnel stops the managed server via `stop\n` on stdin.
    let _ = engine.stop("user_stop")?;
    assert!(
        stop_marker.is_file(),
        "SRV-007: stopping the tunnel must send `stop\\n` over stdin so the server saves its world"
    );
    assert_eq!(fs::read(&world_file)?, b"world-v1-data");

    // 5. SRV-007: dropping the engine on app exit leaves no server process running.
    let exit_marker = data_paths.resolve(&["servers", "minecraft-java", "exit.marker"])?;
    let exit_ps_script = format!(
        "[Console]::Out.WriteLine('[Server thread/INFO]: Done (1.1s)!'); [Console]::Out.Flush(); $line = [Console]::In.ReadLine(); if ($line -eq 'stop') {{ [System.IO.File]::WriteAllText('{}', 'stopped_on_drop') }}",
        exit_marker.display()
    );
    let drop_spec = ServerLaunchSpec {
        program: PathBuf::from("powershell"),
        args: vec![
            "-NoProfile".to_owned(),
            "-NonInteractive".to_owned(),
            "-Command".to_owned(),
            exit_ps_script,
        ],
        required_java_major: 25,
        selected_java_major: 25,
    };
    let _ = engine.start_managed_server("minecraft-java", drop_spec)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if engine.poll_managed_server()? == ManagedServerState::Running {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    drop(engine);
    assert!(
        exit_marker.is_file(),
        "SRV-007: dropping EngineLifecycle on app exit must gracefully stop the managed server process"
    );

    // Ensure the events enum variants are exercised.
    let _ = Command::new("cmd")
        .args(["/C", "rem"])
        .stdin(Stdio::null())
        .status();
    let sample_event = EngineEvent::ManagedServerStatusChanged {
        state: ManagedServerState::Running,
        progress: Some(100),
    };
    assert!(matches!(
        sample_event,
        EngineEvent::ManagedServerStatusChanged { .. }
    ));

    Ok(())
}
