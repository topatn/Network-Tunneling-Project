//! Randomized fuzz harness for untrusted protocol inputs on the relay side (`T-094`, `NET-007`).
//!
//! ## What this is, and what it is not
//!
//! This is a **std-only, seeded, mutation-based random fuzzer** run as an ordinary integration
//! test. It is **not coverage-guided** (no `cargo-fuzz`/libFuzzer): that would need a nightly
//! toolchain and a new tool install, which `AGENTS.md` §5/§9.6 do not allow without asking. A
//! coverage-guided run remains a recorded follow-up in `tasks.md` T-094.
//!
//! ## Targets
//!
//! 1. `decode_control_frame` — random bytes, mutated valid frames, hostile length prefixes.
//! 2. `TcpOpenHeader::decode` and `TcpOpenStatus::from_byte` — every byte and random headers.
//! 3. `UdpDatagram::decode` — random frames across random datagram limits.
//! 4. `verify_allocation_token` — mutated valid tokens.
//! 5. `RelayNode::accept_hello_frame` — mutated `hello` frames, with a **second** reservation
//!    whose state must not change (cross-session check).
//! 6. `RelayUdpSession::handle_engine_datagram` and `RelayTcpSession::complete_tcp_open` — fuzzed
//!    on session A while session B's metrics and flow table must stay identical.
//!
//! ## Pass criteria (`T-094` Done when)
//!
//! * **No panic:** every target call is wrapped in `catch_unwind`; a panic fails the test and
//!   prints the seed and the offending input as hex so it can be replayed.
//! * **No hang:** each single call must finish within [`MAX_CALL_DURATION`]; the slowest call is
//!   printed.
//! * **No cross-session effect:** session B state is compared before and after.
//!
//! Override with `GTT_FUZZ_SEED` and `GTT_FUZZ_ITERS`.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::{Duration, Instant};

use gtt_protocol::{
    decode_control_frame, encode_control_frame, encode_udp_datagram, verify_allocation_token,
    AllocationTokenClaims, ControlMessage, FlowSummaryMessage, GoodbyeMessage, HelloAckMapping,
    HelloAckMessage, HelloMessage, KeyId, Protocol, RejectCode, RejectMessage, SequenceMessage,
    SessionLimits, SigningKeyMaterial, SigningKeyRing, TcpOpenHeader, TcpOpenStatus,
    TokenRefreshMessage, UdpDatagram, CURRENT_PROTOCOL_VERSION, DEFAULT_MAX_DATAGRAM_BYTES,
    DEFAULT_TOKEN_ISSUER, MAX_CONTROL_FRAME_BYTES,
};
use gtt_relay::{
    AllocationReservation, QuicConnectionMeta, RelayConfig, RelayNode, RelayTcpSession,
    RelayUdpSession, ReservationState, RELAY_ALPN,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Upper bound for a single target call. Generous so a loaded CI runner does not flake; a real
/// hang is unbounded and still caught.
const MAX_CALL_DURATION: Duration = Duration::from_millis(250);
const DEFAULT_SEED: u64 = 0x6774_745f_6675_7a7a; // "gtt_fuzz"
const DEFAULT_ITERS: u64 = 20_000;

const RELAY_ID: &str = "11111111-1111-4111-8111-111111111111";
const TUNNEL_A: &str = "33333333-3333-4333-8333-333333333331";
const TUNNEL_B: &str = "33333333-3333-4333-8333-333333333332";
const ALLOC_A: &str = "44444444-4444-4444-8444-444444444441";
const ALLOC_B: &str = "44444444-4444-4444-8444-444444444442";
const SESSION_ID: &str = "55555555-5555-4555-8555-555555555555";
const NOW_EPOCH_S: i64 = 1_760_000_000;

/// xorshift64* — deterministic, std-only, good enough for input generation.
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

/// Apply 1–4 random mutations: bit flip, byte set, truncate, insert, duplicate a slice, or
/// rewrite the 4-byte length prefix with a hostile value.
fn mutate(rng: &mut Rng, input: &[u8]) -> Vec<u8> {
    let mut out = input.to_vec();
    let rounds = 1 + rng.below(4);
    for _ in 0..rounds {
        match rng.below(7) {
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
            4 if !out.is_empty() => {
                let start = rng.below(out.len());
                let end = (start + rng.below(64)).min(out.len());
                let slice: Vec<u8> = out.get(start..end).map(<[u8]>::to_vec).unwrap_or_default();
                out.extend_from_slice(&slice);
            }
            5 if out.len() >= 4 => {
                let hostile: [u32; 6] = [
                    0,
                    1,
                    u32::MAX,
                    u32::try_from(MAX_CONTROL_FRAME_BYTES).unwrap_or(u32::MAX),
                    u32::try_from(MAX_CONTROL_FRAME_BYTES + 1).unwrap_or(u32::MAX),
                    u32::try_from(out.len()).unwrap_or(0).saturating_add(7),
                ];
                let v = hostile
                    .get(rng.below(hostile.len()))
                    .copied()
                    .unwrap_or(0)
                    .to_be_bytes();
                for (dst, src) in out.iter_mut().zip(v.iter()) {
                    *dst = *src;
                }
            }
            _ => {
                // JSON-shaped noise in the body so the parser sees near-valid structure.
                let fragments: [&[u8]; 8] = [
                    b"{",
                    b"}",
                    b"\"type\":",
                    b"\"local_host\":\"127.0.0.1\"",
                    b"[[[[[[[[[[[[[[[[",
                    b"\\u0000",
                    b"1e999999",
                    b"\xff\xfe",
                ];
                let frag = fragments
                    .get(rng.below(fragments.len()))
                    .copied()
                    .unwrap_or(b"");
                let i = rng.below(out.len().saturating_add(1));
                let tail = out.split_off(i.min(out.len()));
                out.extend_from_slice(frag);
                out.extend_from_slice(&tail);
            }
        }
    }
    out
}

/// Run `f(input)` under `catch_unwind` and a per-call time bound, recording the slowest call.
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

fn mappings() -> Vec<HelloAckMapping> {
    vec![
        HelloAckMapping {
            index: 0,
            mapping_key: "game".to_owned(),
            protocol: Protocol::Tcp,
            public_port: 25565,
        },
        HelloAckMapping {
            index: 1,
            mapping_key: "query".to_owned(),
            protocol: Protocol::Udp,
            public_port: 25566,
        },
    ]
}

fn claims(aid: &str, sub: &str, jti: &str) -> AllocationTokenClaims {
    AllocationTokenClaims {
        iss: DEFAULT_TOKEN_ISSUER.to_owned(),
        aud: RELAY_ID.to_owned(),
        sub: sub.to_owned(),
        aid: aid.to_owned(),
        mappings: mappings(),
        iat: NOW_EPOCH_S - 10,
        exp: NOW_EPOCH_S + 300,
        jti: jti.to_owned(),
    }
}

fn ring() -> Result<SigningKeyRing, Box<dyn std::error::Error>> {
    Ok(SigningKeyRing::new(
        DEFAULT_TOKEN_ISSUER,
        KeyId::new("kid-fuzz")?,
        SigningKeyMaterial::from_bytes([0x5a; 32])?,
        NOW_EPOCH_S - 3600,
    ))
}

fn seed_corpus(token: &str) -> Result<Vec<Vec<u8>>, Box<dyn std::error::Error>> {
    let msgs = vec![
        ControlMessage::Hello(HelloMessage {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            token: token.to_owned(),
            client_version: "0.1.0-fuzz".to_owned(),
        }),
        ControlMessage::HelloAck(HelloAckMessage {
            session_id: SESSION_ID.to_owned(),
            heartbeat_interval_ms: 5_000,
            idle_timeout_ms: 15_000,
            limits: SessionLimits {
                max_tcp_flows: 8,
                max_udp_flows: 8,
            },
            mappings: mappings(),
        }),
        ControlMessage::Reject(RejectMessage {
            code: RejectCode::Capacity,
            retry_after_ms: Some(1_000),
        }),
        ControlMessage::Ping(SequenceMessage { seq: 7 }),
        ControlMessage::Pong(SequenceMessage { seq: 7 }),
        ControlMessage::TokenRefresh(TokenRefreshMessage {
            token: token.to_owned(),
        }),
        ControlMessage::FlowSummary(FlowSummaryMessage {
            tcp_flows: 2,
            udp_flows: 3,
            distinct_sources: 4,
        }),
        ControlMessage::Goodbye(GoodbyeMessage {
            reason: "user_stop".to_owned(),
        }),
    ];
    let mut corpus = Vec::with_capacity(msgs.len());
    for m in &msgs {
        corpus.push(encode_control_frame(m)?);
    }
    Ok(corpus)
}

#[test]
fn fuzz_relay_side_protocol_inputs_without_panic_hang_or_cross_session_effect() -> TestResult {
    let seed = env_u64("GTT_FUZZ_SEED", DEFAULT_SEED);
    let iters = env_u64("GTT_FUZZ_ITERS", DEFAULT_ITERS);
    let mut rng = Rng::new(seed);
    let mut slowest = Duration::ZERO;
    let run_start = Instant::now();

    let ring = ring()?;
    let key_set = ring.export_verification_keys(NOW_EPOCH_S);
    let valid_token = ring.sign_allocation_token(&claims(
        ALLOC_A,
        TUNNEL_A,
        "66666666-6666-4666-8666-666666666661",
    ))?;
    let corpus = seed_corpus(&valid_token)?;

    // Relay node with two reservations; B must never change while A is fuzzed.
    let mut relay = RelayNode::new(RelayConfig::new(RELAY_ID), key_set.clone())?;
    for (aid, sub) in [(ALLOC_A, TUNNEL_A), (ALLOC_B, TUNNEL_B)] {
        relay.upsert_reservation(AllocationReservation {
            allocation_id: aid.to_owned(),
            tunnel_id: sub.to_owned(),
            expires_at_epoch_s: NOW_EPOCH_S + 600,
            mappings: mappings(),
            state: ReservationState::Reserved,
        })?;
    }
    let reservation_b_before = relay.reservation(ALLOC_B).cloned();
    let conn = QuicConnectionMeta {
        tls_authenticated: true,
        negotiated_alpn: RELAY_ALPN.to_owned(),
        assigned_session_id: SESSION_ID.to_owned(),
    };

    // Data-path sessions A (fuzzed) and B (observer).
    let mut udp_a = RelayUdpSession::new(&mappings(), 8, DEFAULT_MAX_DATAGRAM_BYTES, 30_000);
    let mut udp_b = RelayUdpSession::new(&mappings(), 8, DEFAULT_MAX_DATAGRAM_BYTES, 30_000);
    let guest_b: std::net::SocketAddr = "198.51.100.9:40000".parse()?;
    let _ = udp_b
        .handle_guest_datagram(1, guest_b, b"seed", 0)
        .ok_or("seeding session B flow failed")?;
    let udp_b_metrics_before = *udp_b.metrics();
    let udp_b_flow_before = udp_b.flow_id_for_source(1, guest_b);
    // Give A a real flow too, so fuzzed frames can hit a known flow id.
    let guest_a: std::net::SocketAddr = "203.0.113.5:40001".parse()?;
    let _ = udp_a.handle_guest_datagram(1, guest_a, b"seed", 0);
    let flow_a = udp_a
        .flow_id_for_source(1, guest_a)
        .ok_or("seeding session A flow failed")?;
    let udp_a_valid = encode_udp_datagram(1, flow_a, b"reply", DEFAULT_MAX_DATAGRAM_BYTES)?;
    let mut udp_a_accepted = 0_u64;

    let mut tcp_a = RelayTcpSession::new(&mappings(), 8);
    let tcp_b = RelayTcpSession::new(&mappings(), 8);
    let tcp_b_metrics_before = *tcp_b.metrics();

    // Target 2a: exhaustive over every status byte.
    for b in 0_u8..=255 {
        run_guarded("TcpOpenStatus::from_byte", seed, &[b], &mut slowest, |i| {
            let _ = TcpOpenStatus::from_byte(i.first().copied().unwrap_or(0));
        })?;
    }

    let mut counts = [0_u64; 6];
    for n in 0..iters {
        // Alternate between pure random input and mutations of a valid corpus entry.
        let base = corpus
            .get(rng.below(corpus.len()))
            .cloned()
            .unwrap_or_default();
        let input = if n % 4 == 0 {
            rng.bytes(512)
        } else {
            mutate(&mut rng, &base)
        };

        match n % 6 {
            0 => {
                run_guarded("decode_control_frame", seed, &input, &mut slowest, |i| {
                    let _ = decode_control_frame(i);
                })?;
            }
            1 => {
                run_guarded("TcpOpenHeader::decode", seed, &input, &mut slowest, |i| {
                    let _ = TcpOpenHeader::decode(i);
                })?;
            }
            2 => {
                let limit = rng.below(DEFAULT_MAX_DATAGRAM_BYTES + 16);
                run_guarded("UdpDatagram::decode", seed, &input, &mut slowest, |i| {
                    let _ = UdpDatagram::decode(i, limit);
                })?;
            }
            3 => {
                let token =
                    String::from_utf8_lossy(&mutate(&mut rng, valid_token.as_bytes())).into_owned();
                run_guarded(
                    "verify_allocation_token",
                    seed,
                    token.as_bytes(),
                    &mut slowest,
                    |_| {
                        let _ = verify_allocation_token(
                            &token,
                            &key_set,
                            DEFAULT_TOKEN_ISSUER,
                            RELAY_ID,
                            NOW_EPOCH_S,
                        );
                    },
                )?;
            }
            4 => {
                run_guarded(
                    "RelayNode::accept_hello_frame",
                    seed,
                    &input,
                    &mut slowest,
                    |i| {
                        let _ = relay.accept_hello_frame(&conn, i, NOW_EPOCH_S);
                    },
                )?;
            }
            _ => {
                // Half the time mutate a *valid* datagram for session A's known flow, so the
                // decoder's success path and the flow lookup are exercised, not just rejection.
                let udp_input = if n % 12 == 5 {
                    mutate(&mut rng, &udp_a_valid)
                } else {
                    input.clone()
                };
                run_guarded(
                    "RelayUdpSession::handle_engine_datagram",
                    seed,
                    &udp_input,
                    &mut slowest,
                    |i| {
                        if udp_a.handle_engine_datagram(i, 1).is_some() {
                            udp_a_accepted += 1;
                        }
                    },
                )?;
                let status = input.first().copied().unwrap_or(0);
                run_guarded(
                    "RelayTcpSession::complete_tcp_open",
                    seed,
                    &[status],
                    &mut slowest,
                    |_| {
                        if tcp_a.begin_tcp_open(0, guest_a.ip()).is_ok() {
                            let _ = tcp_a.complete_tcp_open(status);
                            tcp_a.finish_tcp_flow(0, 0);
                        }
                    },
                )?;
            }
        }
        if let Some(c) = counts.get_mut(usize::try_from(n % 6).unwrap_or(0)) {
            *c += 1;
        }
    }

    // Cross-session checks (`NET-007`: sessions of other tunnels are unaffected).
    assert_eq!(relay.reservation(ALLOC_B).cloned(), reservation_b_before);
    assert_eq!(*udp_b.metrics(), udp_b_metrics_before);
    assert_eq!(udp_b.flow_id_for_source(1, guest_b), udp_b_flow_before);
    assert_eq!(*tcp_b.metrics(), tcp_b_metrics_before);
    // Bounded state on the fuzzed side.
    assert!(udp_a.metrics().active_udp_flows <= 8);
    assert!(tcp_a.metrics().active_tcp_flows <= 8);
    assert!(relay.replay_cache().len() <= RelayConfig::new(RELAY_ID).max_replay_entries);
    // Counters moved, i.e. malformed input was actually counted rather than silently ignored,
    // and some mutated datagrams still decoded and reached the flow table.
    assert!(udp_a.metrics().datagrams_dropped > 0);
    assert!(udp_a_accepted > 0);

    println!(
        "T-094 relay fuzz: seed={seed:#x} iters={iters} per_target={counts:?} \
         slowest_call={slowest:?} total={:?} udp_a_dropped={} udp_a_accepted={udp_a_accepted} \
         replay_cache={}",
        run_start.elapsed(),
        udp_a.metrics().datagrams_dropped,
        relay.replay_cache().len()
    );
    Ok(())
}
