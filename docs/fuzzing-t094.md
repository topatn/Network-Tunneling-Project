# Fuzzing of protocol inputs — T-094 results

**Status:** recorded run, 2026-10-24. Requirement: `NET-007`. Task: `tasks.md` T-094.

## Method

The fuzzer is std-only, seeded and mutation-based. It runs as two ordinary integration tests:

- `crates/relay/tests/fuzz_protocol_inputs.rs`: `decode_control_frame`, `TcpOpenHeader::decode`, `TcpOpenStatus::from_byte` (all 256 bytes), `UdpDatagram::decode`, `verify_allocation_token`, `RelayNode::accept_hello_frame`, `RelayUdpSession::handle_engine_datagram`, `RelayTcpSession::complete_tcp_open`.
- `crates/engine/tests/fuzz_relay_inputs.rs`: `EngineLifecycle::handle_relay_control_frame`, `EngineTcpSession::evaluate_open_header`, `EngineUdpSession::handle_relay_datagram`.

Inputs are a mix of pure random bytes and mutations of valid frames: bit flips, byte overwrites, truncation, insertion, slice duplication, hostile 4-byte length prefixes, and JSON fragments, including attempts to inject `local_host`/`local_port`.

Pass criteria checked on every call:

- **No panic.** Each call runs under `catch_unwind`. A failure prints the seed and the input as hex.
- **No hang.** Each call must finish in 250 ms or less.
- **No cross-session effect.** For each target there is a second observer session: reservation B, UDP session B, and TCP session B. Its state must be identical before and after the run.
- **SEC-006 under fuzzing (engine).**
  - The configured local targets stay byte-for-byte unchanged.
  - A disabled mapping never creates a flow.
  - Every TCP open the fuzzer accepts resolves only to the enabled configured target.

## Limits (read before relying on this)

- **Not coverage-guided.** This is not `cargo-fuzz`/libFuzzer. Those need a nightly toolchain and a new tool install, which `AGENTS.md` §5 and §9.6 do not allow without asking. A coverage-guided run is an open follow-up.
- Runs in-process. It does not go through QUIC or real sockets on the relay side.

## Recorded run

Command (PowerShell):

```
$env:GTT_FUZZ_ITERS='500000'
$env:GTT_FUZZ_SEED='<seed>'
cargo +stable-x86_64-pc-windows-gnu test -p gtt-relay --test fuzz_protocol_inputs -p gtt-engine --test fuzz_relay_inputs -- --nocapture
```

| Harness | Seed | Iterations | Slowest call | Total | Notable counters | Result |
|---|---|---|---|---|---|---|
| relay | `0x7` | 500,000 | 522.4 µs | 19.9 s | udp dropped 71,582; udp accepted 11,751 | pass |
| relay | `0x11f71fb04cb` | 500,000 | 537.7 µs | 20.1 s | udp dropped 71,557; udp accepted 11,776 | pass |
| engine | `0x7` | 500,000 | 265.6 µs | 5.6 s | udp dropped 139,793; tcp accepted 2,221 | pass |
| engine | `0x11f71fb04cb` | 500,000 | 1.54 ms | 5.9 s | udp dropped 142,095; tcp accepted 2,167 | pass |

Across 2,000,000 total calls there were no panics, no hang-suspect calls, and no cross-session changes. The default CI run is 20,000 iterations per harness with fixed seeds.
