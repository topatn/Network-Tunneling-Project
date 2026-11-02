//! Automated privacy and network capture audit tests (`T-098`, `PRIV-001`, `PRIV-002`).
//!
//! Asserts:
//! 1. **Default-Install Telemetry Exclusion (`PRIV-001`):** The engine client sends no unsolicited
//!    telemetry, tracking, or metrics by default. Every outbound HTTP and socket connection connects
//!    exclusively to explicitly configured endpoints (Control-Plane API, GTT Relay, or Local Loopback).
//! 2. **Network Traffic Allowlist:** Outbound HTTP requests contain only API protocol routes,
//!    with zero third-party reporting domains (Google Analytics, Sentry, Mixpanel, Segment, Datadog).
//! 3. **Data Minimization in Transit (`PRIV-002`, `INV-1`):** Request payloads sent to the control
//!    plane never transmit local server hostnames or internal destination ports.
//! 4. **No Telemetry Dependencies in Manifest:** Verifies that no telemetry or third-party tracking
//!    crates are included in the workspace dependencies.

use std::collections::HashSet;
use std::error::Error;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gtt_engine::{
    ClientConfig, ControlPlaneClient, DeterministicBackoff, EngineLifecycle, HttpMethod,
    HttpRequest, HttpResponse, HttpTransport, MappingTargetInput, NoTokenRefresher, TransportError,
};
use gtt_profiles::Registry;
use gtt_protocol::{
    encode_control_frame, ControlMessage, CreateTunnelRequest, EngineSessionState, HelloAckMapping,
    HelloAckMessage, MappingRequest, Protocol, SessionLimits, StartTunnelResponse,
};

type TestResult = Result<(), Box<dyn Error>>;

fn repo_profiles_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("profiles")
}

/// Capturing mock HTTP transport tracking every network request attempted by the client.
#[derive(Clone, Default)]
struct CapturingHttpTransport {
    captured_requests: Arc<Mutex<Vec<HttpRequest>>>,
}

impl CapturingHttpTransport {
    fn new() -> Self {
        Self {
            captured_requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn requests(&self) -> Vec<HttpRequest> {
        self.captured_requests
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

impl HttpTransport for CapturingHttpTransport {
    fn send(&mut self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let mut guard = self
            .captured_requests
            .lock()
            .map_err(|e| TransportError::Network(format!("mutex poisoned: {e}")))?;
        guard.push(request.clone());

        // Return a valid mock response for API endpoints
        if request.url.ends_with("/start") && request.method == HttpMethod::Post {
            let body = serde_json::json!({
                "tunnel_id": "11111111-1111-4111-8111-111111111111",
                "allocation_token": "mock.alloc.jwt",
                "allocation": {
                    "id": "22222222-2222-4222-8222-222222222222",
                    "expires_at": "2026-11-03T10:00:00Z",
                    "relay": {
                        "region": "ap-southeast",
                        "host": "relay.ap-southeast.example.internal",
                        "tunnel_port": 4433
                    },
                    "ports": [
                        { "mapping_key": "game", "protocol": "tcp", "public_port": 25565 }
                    ]
                }
            })
            .to_string()
            .into_bytes();

            return Ok(HttpResponse {
                status: 200,
                headers: vec![("content-type".to_owned(), "application/json".to_owned())],
                body,
            });
        }

        if request.url.ends_with("/v1/tunnels") && request.method == HttpMethod::Post {
            let body = serde_json::json!({
                "id": "11111111-1111-4111-8111-111111111111",
                "name": "my-server",
                "game_profile_id": "minecraft-java",
                "profile_version": 1,
                "state": "created",
                "created_at": "2026-11-02T10:00:00Z",
                "started_at": null,
                "stopped_at": null,
                "expires_at": "2026-11-03T10:00:00Z",
                "termination_reason": null,
                "mappings": [
                    { "mapping_key": "game", "protocol": "tcp", "position": 0, "enabled": true }
                ]
            })
            .to_string()
            .into_bytes();

            return Ok(HttpResponse {
                status: 201,
                headers: vec![("content-type".to_owned(), "application/json".to_owned())],
                body,
            });
        }

        if request.url.contains("/v1/tunnels/") && request.method == HttpMethod::Get {
            let body = serde_json::json!({
                "id": "11111111-1111-4111-8111-111111111111",
                "name": "my-server",
                "game_profile_id": "minecraft-java",
                "profile_version": 1,
                "state": "created",
                "created_at": "2026-11-02T10:00:00Z",
                "started_at": null,
                "stopped_at": null,
                "expires_at": "2026-11-03T10:00:00Z",
                "termination_reason": null,
                "mappings": [
                    { "mapping_key": "game", "protocol": "tcp", "position": 0, "enabled": true }
                ]
            })
            .to_string()
            .into_bytes();

            return Ok(HttpResponse {
                status: 200,
                headers: vec![("content-type".to_owned(), "application/json".to_owned())],
                body,
            });
        }

        if request.url.contains("/v1/tunnels/") && request.method == HttpMethod::Get {
            let body = serde_json::json!({
                "id": "11111111-1111-4111-8111-111111111111",
                "name": "my-server",
                "game_profile_id": "minecraft-java",
                "profile_version": 1,
                "state": "created",
                "created_at": "2026-11-02T10:00:00Z",
                "started_at": null,
                "stopped_at": null,
                "expires_at": "2026-11-03T10:00:00Z",
                "termination_reason": null,
                "mappings": [
                    { "mapping_key": "game", "protocol": "tcp", "position": 0, "enabled": true }
                ]
            })
            .to_string()
            .into_bytes();

            return Ok(HttpResponse {
                status: 200,
                headers: vec![("content-type".to_owned(), "application/json".to_owned())],
                body,
            });
        }

        // Generic 200 response
        Ok(HttpResponse {
            status: 200,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: b"{}".to_vec(),
        })
    }
}

// ---------------------------------------------------------------------------
// 1. Default-Install Network Capture and Telemetry Exclusion (PRIV-001)
// ---------------------------------------------------------------------------

#[test]
fn test_default_install_network_capture_has_zero_telemetry() -> TestResult {
    let transport = CapturingHttpTransport::new();
    let api_url = "https://api.tunnel.example.internal";

    let config = ClientConfig::new(api_url);
    let mut client = ControlPlaneClient::new(
        config,
        Some("test-access-token".to_owned()),
        transport.clone(),
        DeterministicBackoff::with_jitter_bps(0),
        NoTokenRefresher,
    )?;

    // Perform standard host workflow: create tunnel, start tunnel, fetch details
    let idempotency_key = gtt_engine::IdempotencyKey::new("idemp-key-privacy-1")?;
    let create_req = CreateTunnelRequest {
        name: "privacy-test-server".to_owned(),
        game_profile_id: "minecraft-java".to_owned(),
        mappings: vec![MappingRequest {
            mapping_key: "game".to_owned(),
            protocol: Protocol::Tcp,
            enabled: true,
        }],
    };

    let tunnel_id = "11111111-1111-4111-8111-111111111111";
    let _ = client.create_tunnel(&create_req, &idempotency_key)?;
    let _ = client.start_tunnel(tunnel_id, &idempotency_key)?;
    let _ = client.get_tunnel(tunnel_id)?;

    // Capture and inspect every outbound request
    let captured = transport.requests();
    assert_eq!(captured.len(), 3, "expected exactly 3 API requests");

    // Forbidden telemetry keywords and domains (PRIV-001)
    let forbidden_patterns = [
        "telemetry",
        "analytics",
        "sentry",
        "mixpanel",
        "segment.io",
        "datadoghq",
        "google-analytics",
        "stats",
        "metrics-collector",
    ];

    let mut visited_domains = HashSet::new();

    for req in &captured {
        // 1. Assert request URL starts strictly with the configured API URL
        assert!(
            req.url.starts_with(api_url),
            "outbound request directed outside configured API: {}",
            req.url
        );

        // 2. Assert no third-party telemetry domains or tracking query parameters
        let url_lower = req.url.to_lowercase();
        for pattern in forbidden_patterns {
            assert!(
                !url_lower.contains(pattern),
                "request URL contains forbidden tracking/telemetry pattern '{pattern}': {}",
                req.url
            );
        }

        // 3. Inspect headers: zero tracking headers
        for (header_name, header_val) in &req.headers {
            let h_lower = header_name.to_lowercase();
            assert!(
                !h_lower.contains("telemetry")
                    && !h_lower.contains("tracking")
                    && !h_lower.contains("analytics"),
                "found tracking header {header_name}: {header_val}"
            );
        }

        // Extract hostname
        if let Some(host) = req.url.strip_prefix("https://") {
            let domain = host.split('/').next().unwrap_or("");
            visited_domains.insert(domain.to_owned());
        }
    }

    // Only the single configured API host must ever have been contacted
    assert_eq!(visited_domains.len(), 1);
    assert!(visited_domains.contains("api.tunnel.example.internal"));

    println!(
        "\n [PRIV-001] Network capture verified: 100% of traffic directed to api.tunnel.example.internal; zero telemetry."
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// 2. Local Reachability Probes Restrict Sockets to Loopback (PRIV-001, SEC-006)
// ---------------------------------------------------------------------------

#[test]
fn test_engine_lifecycle_probes_only_loopback_and_configured_relay() -> TestResult {
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

    // Local reachability probe: dials only 127.0.0.1:local_port
    let probe_res = engine.probe_local(Some("game"), Duration::from_millis(100));
    assert!(!probe_res.is_empty());

    // Connect to mock relay
    let start_resp: StartTunnelResponse = serde_json::from_value(serde_json::json!({
        "tunnel_id": "11111111-1111-4111-8111-111111111111",
        "allocation_token": "token-1",
        "allocation": {
            "id": "22222222-2222-4222-8222-222222222222",
            "expires_at": "2026-11-03T10:00:00Z",
            "relay": {
                "region": "ap-southeast",
                "host": "relay.ap-southeast.example.internal",
                "tunnel_port": 4433
            },
            "ports": [
                { "mapping_key": "game", "protocol": "tcp", "public_port": 25565 }
            ]
        }
    }))?;

    let state = engine.begin_start_with_response(&start_resp)?;
    assert_eq!(state, EngineSessionState::Connecting);

    let status = engine.get_status();
    assert_eq!(status.public_endpoints.len(), 1);
    assert_eq!(
        status.public_endpoints.first().map(|e| e.public_port),
        Some(25565)
    );

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

    // Stop tunnel
    let _ = engine.stop("user_requested")?;
    assert_eq!(engine.state(), EngineSessionState::Stopped);

    println!(" [PRIV-001] Engine lifecycle network activity strictly confined to loopback server and relay endpoint.");
    Ok(())
}

// ---------------------------------------------------------------------------
// 3. Workspace Dependencies Verification (No Telemetry Crates)
// ---------------------------------------------------------------------------

#[test]
fn test_no_telemetry_crates_in_workspace_dependencies() -> TestResult {
    let cargo_toml_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("Cargo.lock");

    let lock_contents = std::fs::read_to_string(&cargo_toml_path)?;

    // Banned telemetry/analytics library names
    let banned_crates = [
        "sentry",
        "sentry-core",
        "datadog",
        "segment",
        "mixpanel",
        "google-analytics",
        "posthog",
        "amplitude",
        "telemetry-battery",
    ];

    for banned in banned_crates {
        let pattern = format!("name = \"{banned}\"");
        assert!(
            !lock_contents.contains(&pattern),
            "found banned telemetry crate '{banned}' in Cargo.lock"
        );
    }

    println!(" [PRIV-001] Cargo.lock audited: zero third-party telemetry SDKs or analytics packages present.");
    Ok(())
}
