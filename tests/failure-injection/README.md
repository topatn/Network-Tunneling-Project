# Failure-Injection Verification Suite (T-097)

Authoritative sources: `requirements.md` (REL-001, REL-002, REL-004), `architecture.md` §3, `contracts.md` §5.6, §8, §11, §12.

## Overview

The failure-injection test suite empirically verifies system resilience under severe infrastructure failures:
1. **Control-Plane Outage (`REL-001`):** Existing tunnels and active guest connections remain fully functional throughout control-plane outages until allocation token credentials expire. The control plane is strictly decoupled from the data forwarding path (0 HTTP queries required during active tunneling). New tunnel creation or start attempts fail gracefully with clear network errors.
2. **Unhealthy Relay Avoidance & Failover Reallocation (`REL-002`):** Unhealthy relays (draining, retired, full, dead, or failing health probes) are skipped by the candidate selection algorithm. If an active relay drops its connection, the engine detects the loss, initiates bounded backoff retries, transitions to terminal `Failed` upon retry exhaustion, and automatically reallocates to an alternative healthy relay with updated endpoints.
3. **Relay Crash and State Recovery (`REL-004`):** When a relay process crashes and restarts with empty memory, it polls the control plane (`GET /internal/v1/relays/{id}/state` with `cursor = None`) to restore its full snapshot of active reservations and guest admissions. Reconnecting engines immediately resume data transmission via `HELLO` handshake without admission rejections or state corruption.

---

## Test Scenarios & Requirements Matrix

| Scenario ID | Requirement | Injected Failure | Expected System Behavior | Verification Target |
|---|---|---|---|---|
| `FAIL-SCENARIO-01` | `REL-001` | Control-plane HTTP API stopped / unreachable | Existing TCP and UDP game forwarding continues without interruption; 0 HTTP requests made. | `gtt-engine::failure_injection::test_control_plane_outage_data_path_survival_and_graceful_failures` |
| `FAIL-SCENARIO-02` | `REL-001` | Control-plane HTTP API stopped / unreachable | New tunnel start requests fail gracefully with descriptive network errors; running sessions remain untouched. | `gtt-engine::failure_injection::test_control_plane_outage_data_path_survival_and_graceful_failures` |
| `FAIL-SCENARIO-03` | `REL-001` | Token credential expiry post-outage | Tunnels survive until allocation token credential expiry (`exp_epoch_s`); expired tokens are rejected. | `gtt-engine::failure_injection::test_control_plane_outage_data_path_survival_and_graceful_failures` |
| `FAIL-SCENARIO-04` | `REL-002` | Relays marked draining, full, or retired | Candidate selection filters out draining, full, and retired nodes, ordering healthy candidates by load. | `gtt-control-plane::tunnels_routes_unit::select_candidate_relays_skips_unhealthy_draining_full_and_excluded_relays_and_orders_by_load` |
| `FAIL-SCENARIO-05` | `REL-002` | Active relay crashes / drops QUIC connection | Engine detects connection loss (`Reconnecting`), attempts 5 bounded reconnects, and transitions to terminal `Failed`. | `gtt-engine::failure_injection::test_unhealthy_relay_avoidance_and_failover_reallocation` |
| `FAIL-SCENARIO-06` | `REL-002` | Primary relay terminal failure | Engine reallocates to a secondary healthy relay (`failover.relay.example.test`), transitions to `Active`, and updates endpoints. | `gtt-engine::failure_injection::test_unhealthy_relay_avoidance_and_failover_reallocation` |
| `FAIL-SCENARIO-07` | `REL-004` | Relay process killed / memory wiped | Restarted relay starts empty, requests full snapshot (`cursor = None`), restores reservations and admissions. | `gtt-control-plane::limiter_audit_relay_unit::relay_restart_recovery_restores_full_snapshot_and_cursor_poll_returns_deltas` |
| `FAIL-SCENARIO-08` | `REL-004` | Engine reconnects after relay restart | Relay accepts engine `HELLO` with previous token, restores `Active` tunnel session, and maintains guest admissions. | `gtt-engine::failure_injection::test_relay_crash_and_restart_recovery_from_control_plane` |

---

## Running the Suite

### Standalone Python Harness
```bash
python tests/failure-injection/run_failure_injection.py
```

### Direct Cargo Invocations
```bash
# Engine failure injection integration suite
cargo test -p gtt-engine --test failure_injection -- --nocapture

# Control-plane candidate selection unit tests
cargo test -p gtt-control-plane --test tunnels_routes_unit select_candidate_relays -- --nocapture

# Relay restart recovery snapshot and delta unit tests
cargo test -p gtt-control-plane --test limiter_audit_relay_unit relay_restart_recovery -- --nocapture
```
