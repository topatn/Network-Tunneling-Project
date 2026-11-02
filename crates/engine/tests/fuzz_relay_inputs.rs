//! Randomized fuzz harness for untrusted inputs on the engine side (`T-094`, `NET-007`,
//! `SEC-006`).
//!
//! Same method and limits as `crates/relay/tests/fuzz_protocol_inputs.rs`: std-only, seeded,
//! mutation-based, **not coverage-guided**. Targets the three places relay-controlled bytes
//! enter the engine:
//!
//! 1. `EngineLifecycle::handle_relay_control_frame` (control stream).
//! 2. `EngineTcpSession::evaluate_open_header` (TCP stream open header).
//! 3. `EngineUdpSession::handle_relay_datagram` (QUIC DATAGRAM frames).
//!
//! Besides no panic and no hang, it checks the `SEC-006` invariant under fuzzing: whatever the
//! relay sends, the engine's configured local targets are unchanged, and a disabled mapping is
//! never dialed. A second, independent `EngineUdpSession` must be unaffected.
//!
//! Override with `GTT_FUZZ_SEED` and `GTT_FUZZ_ITERS`.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gtt_engine::{
    EngineLifecycle, EngineTcpSession, EngineUdpSession, LocalMappingConfig, MappingTargetInput,
    DEFAULT_ENGINE_TCP_IO_TIMEOUT, DEFAULT_LOCAL_DIAL_TIMEOUT,
};
use gtt_profiles::Registry;
use gtt_protocol::{
    encode_control_frame, encode_udp_datagram, ControlMessage, FlowSummaryMessage, HelloAckMapping,
    HelloAckMessage, Protocol, SequenceMessage, SessionLimits, StartTunnelResponse, TcpOpenHeader,
    DEFAULT_MAX_DATAGRAM_BYTES,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const MAX_CALL_DURATION: Duration = Duration::from_millis(250);
const DEFAULT_SEED: u64 = 0x656e_675f_6675_7a7a; // "eng_fuzz"
const DEFAULT_ITERS: u64 = 20_000;

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(if seed == 0 { DEFAULT_SEED } else { seed })
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            usize::try_from(self.next_u64() % u64::try_from(n).unwrap_or(u64::MAX)).unwrap_or(0)
        }
    }
    fn byte(&mut self) -> u8 {
        self.next_u64().to_le_bytes().first().copied().unwrap_or(0)
    }
    fn bytes(&mut self, max_len: usize) -> Vec<u8> {
        let len = self.below(max_len.saturating_add(1));
        (0..len).map(|_| self.byte()).collect()
    }
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn mutate(rng: &mut Rng, input: &[u8]) -> Vec<u8> {
    let mut out = input.to_vec();
    for _ in 0..(1 + rng.below(4)) {
        match rng.below(5) {
            0 if !out.is_empty() => {
                let i = rng.below(out.len());
                if let Some(b) = out.get_mut(i) {
                    *b ^= 1 << rng.below(8);
                }
            }
            1 if !out.is_empty() => {
                let i = rng.below(out.len());
                let v = rng.byte();
                if let Some(b) = out.get_mut(i) {
                    *b = v;
                }
            }
            2 => out.truncate(rng.below(out.len().saturating_add(1))),
            3 => {
                let i = rng.below(out.len().saturating_add(1));
                let extra = rng.bytes(16);
                let tail = out.split_off(i.min(out.len()));
                out.extend_from_slice(&extra);
                out.extend_from_slice(&tail);
            }
            _ => {
                // Try to smuggle a local destination into control JSON (`INV-1`).
                let frag: &[u8] = b",\"local_host\":\"10.0.0.1\",\"local_port\":22";
                let i = rng.below(out.len().saturating_add(1));
                let tail = out.split_off(i.min(out.len()));
                out.extend_from_slice(frag);
                out.extend_from_slice(&tail);
            }
        }
    }
    out
}

fn run_guarded<F: FnMut(&[u8])>(
    target: &str,
    seed: u64,
    input: &[u8],
    slowest: &mut Duration,
    mut f: F,
) -> TestResult {
    let start = Instant::now();
    let outcome = catch_unwind(AssertUnwindSafe(|| f(input)));
    let elapsed = start.elapsed();
    if elapsed > *slowest {
        *slowest = elapsed;
    }
    let hex: String = input.iter().map(|b| format!("{b:02x}")).collect();
    if outcome.is_err() {
        return Err(format!("PANIC in {target} (seed {seed:#x}) on input {hex}").into());
    }
    if elapsed > MAX_CALL_DURATION {
        return Err(format!(
            "HANG-suspect in {target} (seed {seed:#x}): {elapsed:?} on input {hex}"
        )
        .into());
    }
    Ok(())
}

fn udp_config(index: u16, key: &str, addr: SocketAddr, enabled: bool) -> LocalMappingConfig {
    LocalMappingConfig {
        index,
        mapping_key: key.to_owned(),
        protocol: Protocol::Udp,
        enabled,
        local_addr: addr,
        connect_timeout: DEFAULT_LOCAL_DIAL_TIMEOUT,
        io_timeout: DEFAULT_ENGINE_TCP_IO_TIMEOUT,
    }
}

#[test]
fn fuzz_engine_side_relay_inputs_without_panic_hang_or_target_change() -> TestResult {
    let seed = env_u64("GTT_FUZZ_SEED", DEFAULT_SEED);
    let iters = env_u64("GTT_FUZZ_ITERS", DEFAULT_ITERS);
    let mut rng = Rng::new(seed);
    let mut slowest = Duration::ZERO;
    let run_start = Instant::now();

    // --- 1. Lifecycle control stream -------------------------------------------------------
    let profiles = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../profiles");
    let registry = Registry::load_dir(&profiles)?;
    let mc_java = registry
        .get("minecraft-java")
        .ok_or("missing minecraft-java profile")?;
    let inputs = vec![
        MappingTargetInput {
            mapping_key: "game".to_owned(),
            local_host: "127.0.0.1".to_owned(),
            local_port: 25565,
            enabled: true,
            confirm_non_loopback: false,
        },
        MappingTargetInput {
            mapping_key: "rcon".to_owned(),
            local_host: "127.0.0.1".to_owned(),
            local_port: 25575,
            enabled: false,
            confirm_non_loopback: false,
        },
    ];
    let mut engine = EngineLifecycle::configure(mc_java, &inputs, "0.1.0-fuzz")
        .map_err(|e| format!("configure failed: {e:?}"))?;
    let start_resp: StartTunnelResponse = serde_json::from_value(serde_json::json!({
        "tunnel_id": "11111111-1111-4111-8111-111111111111",
        "allocation_token": "eyJhbGciOiJIUzI1NiJ9.eyJpc3MiOiJndHQifQ.c2ln",
        "allocation": {
            "id": "22222222-2222-4222-8222-222222222222",
            "expires_at": "2026-10-20T12:00:00Z",
            "relay": { "region": "us-east", "host": "relay.example.test", "tunnel_port": 4433 },
            "ports": [ { "mapping_key": "game", "protocol": "tcp", "public_port": 25565 } ]
        }
    }))?;
    let _ = engine.begin_start_with_response(&start_resp)?;
    let _ = engine.on_quic_connected()?;

    let targets_before: Vec<(String, SocketAddr, bool)> = engine
        .target_table()
        .targets()
        .iter()
        .map(|t| (t.mapping_key().to_owned(), t.local_addr(), t.enabled()))
        .collect();

    // A relay-supplied hello_ack that tries to bind the disabled `rcon` and a bogus key.
    let ack_corpus = [
        encode_control_frame(&ControlMessage::HelloAck(HelloAckMessage {
            session_id: "3f2b8c1e-5a47-4d0b-9c1e-0a6f1d2e7b44".to_owned(),
            heartbeat_interval_ms: 5_000,
            idle_timeout_ms: 15_000,
            limits: SessionLimits {
                max_tcp_flows: 8,
                max_udp_flows: 8,
            },
            mappings: vec![
                HelloAckMapping {
                    index: 0,
                    mapping_key: "game".to_owned(),
                    protocol: Protocol::Tcp,
                    public_port: 41000,
                },
                HelloAckMapping {
                    index: 1,
                    mapping_key: "rcon".to_owned(),
                    protocol: Protocol::Tcp,
                    public_port: 41001,
                },
            ],
        }))?,
        encode_control_frame(&ControlMessage::Ping(SequenceMessage { seq: 1 }))?,
        encode_control_frame(&ControlMessage::FlowSummary(FlowSummaryMessage {
            tcp_flows: 1,
            udp_flows: 1,
            distinct_sources: 1,
        }))?,
    ];

    // --- 2. TCP open header ----------------------------------------------------------------
    let mut tcp = EngineTcpSession::new(engine.target_table().to_local_mapping_configs(), 8)?;

    // --- 3. UDP datagrams: session A fuzzed, session B observer ----------------------------
    let sink_a = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
    let sink_b = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
    let mut udp_a = EngineUdpSession::new(
        vec![
            udp_config(0, "game", sink_a.local_addr()?, true),
            udp_config(1, "spare", sink_a.local_addr()?, false),
        ],
        4,
        DEFAULT_MAX_DATAGRAM_BYTES,
        30_000,
    )?;
    let mut udp_b = EngineUdpSession::new(
        vec![udp_config(0, "game", sink_b.local_addr()?, true)],
        4,
        DEFAULT_MAX_DATAGRAM_BYTES,
        30_000,
    )?;
    udp_b
        .handle_relay_datagram(
            &encode_udp_datagram(0, 9, b"seed", DEFAULT_MAX_DATAGRAM_BYTES)?,
            0,
        )
        .map_err(|e| format!("seeding session B failed: {e:?}"))?;
    let udp_b_metrics_before = *udp_b.metrics();
    let udp_b_flow_before = udp_b.flow_local_addr(0, 9);
    let udp_valid = encode_udp_datagram(0, 1, b"payload", DEFAULT_MAX_DATAGRAM_BYTES)?;
    let udp_disabled = encode_udp_datagram(1, 1, b"payload", DEFAULT_MAX_DATAGRAM_BYTES)?;

    let mut counts = [0_u64; 3];
    let mut tcp_accepted = 0_u64;
    for n in 0..iters {
        match n % 3 {
            0 => {
                let base = ack_corpus
                    .get(rng.below(ack_corpus.len()))
                    .cloned()
                    .unwrap_or_default();
                let input = if n % 9 == 0 {
                    rng.bytes(512)
                } else {
                    mutate(&mut rng, &base)
                };
                run_guarded(
                    "EngineLifecycle::handle_relay_control_frame",
                    seed,
                    &input,
                    &mut slowest,
                    |i| {
                        let _ = engine.handle_relay_control_frame(i, n);
                    },
                )?;
            }
            1 => {
                let base = TcpOpenHeader {
                    mapping_index: u16::try_from(rng.below(4)).unwrap_or(0),
                }
                .encode();
                let input = if n % 2 == 0 {
                    rng.bytes(8)
                } else {
                    mutate(&mut rng, &base)
                };
                run_guarded(
                    "EngineTcpSession::evaluate_open_header",
                    seed,
                    &input,
                    &mut slowest,
                    |i| {
                        if let Ok(cfg) = tcp.evaluate_open_header(i) {
                            // SEC-006: only an enabled, configured TCP target can come back.
                            assert!(cfg.enabled);
                            assert_eq!(cfg.protocol, Protocol::Tcp);
                            assert_eq!(cfg.local_addr.port(), 25565);
                            tcp_accepted += 1;
                            tcp.record_local_dial_failure();
                        }
                    },
                )?;
            }
            _ => {
                let base = if n % 2 == 0 {
                    &udp_valid
                } else {
                    &udp_disabled
                };
                let input = if n % 5 == 0 {
                    rng.bytes(64)
                } else {
                    mutate(&mut rng, base)
                };
                run_guarded(
                    "EngineUdpSession::handle_relay_datagram",
                    seed,
                    &input,
                    &mut slowest,
                    |i| {
                        let _ = udp_a.handle_relay_datagram(i, n);
                    },
                )?;
            }
        }
        if let Some(c) = counts.get_mut(usize::try_from(n % 3).unwrap_or(0)) {
            *c += 1;
        }
    }

    // SEC-006 under fuzzing: configured local targets are byte-for-byte unchanged.
    let targets_after: Vec<(String, SocketAddr, bool)> = engine
        .target_table()
        .targets()
        .iter()
        .map(|t| (t.mapping_key().to_owned(), t.local_addr(), t.enabled()))
        .collect();
    assert_eq!(targets_after, targets_before);
    // The disabled UDP mapping never created a flow.
    for flow_id in 0..64 {
        assert!(udp_a.flow_local_addr(1, flow_id).is_none());
    }
    // Bounded state and counted drops.
    assert!(udp_a.metrics().active_udp_flows <= 4);
    assert!(udp_a.metrics().datagrams_dropped > 0);
    // The success path was reached, so the SEC-006 asserts above actually ran.
    assert!(tcp_accepted > 0);
    // Cross-session: observer session unaffected.
    assert_eq!(*udp_b.metrics(), udp_b_metrics_before);
    assert_eq!(udp_b.flow_local_addr(0, 9), udp_b_flow_before);

    println!(
        "T-094 engine fuzz: seed={seed:#x} iters={iters} per_target={counts:?} \
         slowest_call={slowest:?} total={:?} udp_a_dropped={} udp_a_active={} \
         tcp_accepted={tcp_accepted} tcp_refused={}",
        run_start.elapsed(),
        udp_a.metrics().datagrams_dropped,
        udp_a.metrics().active_udp_flows,
        tcp.metrics().total_refused
    );
    Ok(())
}
