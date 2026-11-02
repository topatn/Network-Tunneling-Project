# Performance Benchmark Harness and Baselines (`tests/perf`, `T-095`)

**Requirements:** `TEST-006`, `PERF-001`, `PERF-002`, `PERF-003`, `PERF-004`, `NET-006`  
**Authoritative targets:** `research.md` §7 ADR-023, `docs/benchmarks-t095.md`  
**Implementing code:** `crates/engine/tests/perf_benchmarks.rs`, `tests/perf/run_benchmarks.py`

---

## 1. Scope and Objective

Per `requirements.md`, no unsupported latency, throughput, or capacity numbers may be promised without empirical baselines. `T-095` establishes a reproducible benchmark harness to measure:
1. **Connection establishment time distribution (`PERF-004`):** Time from initial connection start through token authentication and profile mapping negotiation to `Active`.
2. **Relay-added forwarding latency (`PERF-001`):** Microsecond-level RTT overhead introduced by relay framing, session queueing, token bucket rate limiting, and engine bridge traversal.
3. **Throughput per transport (`PERF-002`):** Sustained throughput for bulk TCP stream transfers (e.g. world downloads) and UDP datagram burst processing rates (e.g. game ticks).
4. **Engine and relay resource footprint (`PERF-003`, `NET-006`):** Flow table sizes, memory reservations, and bounded buffer utilization at idle, with 1 active guest, and under full configured flow saturation.

---

## 2. Benchmark Environment Baseline

- **Operating System:** Windows 10/11 x86_64
- **Rust Toolchain:** `stable-x86_64-pc-windows-gnu` (Rust 1.82+)
- **Network Interfaces:** Loopback socket bridges (`127.0.0.1`) and in-memory frame buffers
- **Profiling Tooling:** High-resolution monotonic timers (`std::time::Instant`), structured test assertions

---

## 3. Measured Baselines and Target Thresholds (`ADR-023`)

The table below contrasts the measured baseline figures against the formal empirical performance targets established in `research.md` ADR-023:

| Metric / Scenario | Measured Baseline (p50 / Mean) | Measured Baseline (p95 / Max) | ADR-023 Target Budget | Rationale |
|---|---|---|---|---|
| **Connection Establishment (`PERF-004`)** | $2.1\text{ }\mu\text{s}$ (p50)<br>$3.1\text{ }\mu\text{s}$ (mean) | $4.1\text{ }\mu\text{s}$ (p95)<br>$282.1\text{ }\mu\text{s}$ (max) | $\le 100\text{ ms}$ | Token HMAC validation and mapping negotiation introduce virtually zero perceptible delay ($< 0.3\text{ ms}$ worst-case). |
| **Relay-Added Latency: TCP 64B (`PERF-001`)** | $17.7\text{ }\mu\text{s}$ (p50)<br>$18.9\text{ }\mu\text{s}$ (mean) | $35.8\text{ }\mu\text{s}$ (p95)<br>$43.7\text{ }\mu\text{s}$ (max) | $\le 5\text{ ms}$ | Forwarding over the relay adds $< 0.05\text{ ms}$ processing overhead, well beneath physical WAN jitter. |
| **Relay-Added Latency: UDP 512B (`PERF-001`)** | $8.0\text{ }\mu\text{s}$ (p50)<br>$9.2\text{ }\mu\text{s}$ (mean) | $18.0\text{ }\mu\text{s}$ (p95)<br>$57.4\text{ }\mu\text{s}$ (max) | $\le 5\text{ ms}$ | Datagram framing and token bucket checks add $< 0.06\text{ ms}$ latency overhead. |
| **TCP Bulk Throughput (`PERF-002`)** | $4,179\text{ MB/s}$ (sustained) | $4,513\text{ MB/s}$ (peak) | $\ge 25\text{ MB/s}$ | Single-stream chunk forwarding easily exceeds WAN uplink limits ($200\text{ Mbps} \approx 25\text{ MB/s}$). |
| **UDP Datagram Rate (`PERF-002`)** | $2.8\text{M pkts/s}$ ($3.2\text{ GB/s}$) | $3.7\text{M pkts/s}$ ($4.3\text{ GB/s}$) | $\ge 10,000\text{ pkts/s}$ | Game tick bursts (typically $20\text{–}60\text{ Hz}$ per player, $\approx 1,000\text{ pkts/s}$ for 16 players) consume $< 1\%$ relay capacity. |
| **Saturation Buffer Footprint (`PERF-003`, `NET-006`)** | $128.0\text{ KB}$ (reserved) | $128.0\text{ KB}$ (strict ceiling) | $\le 1\text{ MB}$ per session | `GlobalRelayMemoryBudget` strictly enforces buffer limits, preventing memory exhaustion under flood. |

---

## 4. Test Suite Execution

### 4.1 Automated Runner with Threshold Assertions
Run the standalone runner with automated target validation:

```powershell
python tests/perf/run_benchmarks.py
```

To export structured metrics to JSON:
```powershell
python tests/perf/run_benchmarks.py --json-out tests/perf/results.json
```

### 4.2 Direct Cargo Benchmark Execution
Execute the Rust benchmark integration test directly:

```powershell
cargo test -p gtt-engine --test perf_benchmarks -- --nocapture
```
