# Privacy Test Suite (`T-098`)

Automated tests and empirical verification for user privacy, zero telemetry, structured log redaction, data retention schedules, and user account deletion.

## Requirements Covered

| Requirement | Description | Authoritative Source |
|---|---|---|
| `PRIV-001` | **Zero telemetry by default**: client runs without network beaconing, background analytics, or unauthorized third-party network egress. | `requirements.md` §6.1, `context.md` |
| `PRIV-002` | **Data minimization and log redaction**: logs, diagnostics, and audit events never contain tokens, secrets, or raw guest IPs; owner views show masked IPs (`/24` IPv4, `/48` IPv6). | `requirements.md` §6.2, `contracts.md` §5.13, `AGENTS.md` §7 |
| `PRIV-003` | **Data retention schedules**: time-bound automated retention sweeping across all 7 operational data classes. | `requirements.md` §6.3, `contracts.md` §13, `data-model.md` |
| `PRIV-004` | **User account deletion**: immediate termination of active resources (tunnels, reservations, auth sessions) upon deletion request, followed by complete cascading purge. | `requirements.md` §6.4, `contracts.md` §5.14 |

---

## Scenarios & Test Architecture

The privacy verification suite combines integration test suites and static dependency audits:

1. **`PRIV-SCENARIO-01` (`PRIV-001`) — Default-Install Network Capture:**
   - Captured in [`crates/engine/tests/privacy_network_capture.rs`](../../crates/engine/tests/privacy_network_capture.rs).
   - Simulates full desktop client lifecycle (`sign_in`, `create_tunnel`, `start_tunnel`, `stop_tunnel`, `list_mappings`).
   - Asserts that 100% of outbound HTTP requests are sent strictly to the configured control-plane API base URL.
   - Asserts that zero telemetry headers, zero tracking query parameters, and zero third-party endpoints are dialed.

2. **`PRIV-SCENARIO-02` (`PRIV-001`, `SEC-006`) — Strictly-Scoped Engine Probes:**
   - Verifies that engine local game server probes connect exclusively to local loopback (`127.0.0.1` / `::1`) or confirmed LAN/Tailnet interfaces (`100.64.0.0/10`).
   - Verifies that the relay control stream connects exclusively to the explicitly configured relay endpoint (`contracts.md` §10).

3. **`PRIV-SCENARIO-03` & `PRIV-SCENARIO-04` (`PRIV-001`) — Zero Telemetry Dependencies & Domains:**
   - Scans [`Cargo.lock`](../../Cargo.lock) and workspace `Cargo.toml` files for banned telemetry crates (Sentry, Datadog, Segment, Mixpanel, Amplitude, PostHog, Google Analytics, etc.).
   - Scans all Rust source files in `crates/` for hardcoded tracking or beaconing URLs.

4. **`PRIV-SCENARIO-05` (`PRIV-002`) — Structured Redaction & Masking:**
   - Located in [`crates/control-plane/tests/limiter_audit_relay_unit.rs`](../../crates/control-plane/tests/limiter_audit_relay_unit.rs) and [`crates/control-plane/src/redaction.rs`](../../crates/control-plane/src/redaction.rs).
   - Verifies `mask_ip_for_owner`: IPv4 addresses masked to `/24` (`203.0.113.195` -> `203.0.113.0/24`) and IPv6 masked to `/48` (`2001:db8:85a3::...` -> `2001:db8:85a3::/48`).
   - Verifies that structured JSON logs and plain-text lines with `Authorization: Bearer <token>` or `secret=<val>` have sensitive fields redacted to `[REDACTED-SECRET]`, `[REDACTED-TOKEN]`, or `[REDACTED-IP]`.
   - Confirms automated leak scanner detects zero remaining sensitive tokens.

5. **`PRIV-SCENARIO-06` (`PRIV-003`) — Data Retention Schedule Verification:**
   - Validates that retention cutoff calculations match all 7 schedules:
     1. Guest admissions: 24 hours post-expiry/revocation.
     2. Idempotency keys: 24 hours.
     3. Inactive / expired auth sessions: 30 days.
     4. Stopped / terminated tunnels: 30 days.
     5. Aggregated usage records: 90 days.
     6. Audit log events: 90 days.
     7. Soft-deleted user accounts: 30 days.
   - Comprehensive database retention sweep verified in [`crates/control-plane/tests/invariants_db.rs`](../../crates/control-plane/tests/invariants_db.rs).

6. **`PRIV-SCENARIO-07` (`PRIV-004`) — Account Deletion & Cascading Purge:**
   - Validates `soft_delete_user` atomic operations:
     - User marked `deleted_at = now()`, immediately invisible to `get_user`.
     - Active/created tunnels immediately transitioned to `state = 'terminated'` with reason `'account deleted'`.
     - Relay allocations transitioned to `'released'` and allocation ports freed.
     - Auth sessions revoked immediately (`revoked_at = now()`).
   - Validates that fast-forwarding past the 30-day retention cutoff and running `purge_soft_deleted_users` cascades deletion across all user data (`users`, `tunnels`, `tunnel_mappings`, `relay_allocations`, `invitations`, `guest_admissions`, `auth_sessions`).

---

## Running the Tests

To run the complete privacy verification suite:

```powershell
python tests/privacy/run_privacy_tests.py
```

To run individual Rust test suites:

```powershell
# Engine network capture and telemetry dependency checks
cargo test -p gtt-engine --test privacy_network_capture

# Control plane log redaction, retention schedules, and account deletion
cargo test -p gtt-control-plane --test limiter_audit_relay_unit -- t098
```
