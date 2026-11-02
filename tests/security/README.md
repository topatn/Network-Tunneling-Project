# Security Test Suite (T-093 / TEST-004)

**Status:** COMPLETE & VERIFIED.
**Requirements:** `TEST-004`, `SEC-002`, `SEC-005`, `SEC-006`, `SEC-008`, `SEC-009`, `SEC-010`, `SEC-013`, `SEC-015`, `SEC-018`.
**Authoritative References:** `architecture.md` §3, `contracts.md` §2, §3, §4, §5.1, §9, `data-model.md` §4, `research.md` ADR-003, ADR-018, ADR-019.

---

## 1. Scope & Purpose

`TEST-004` requires an automated security test suite covering eight mandatory attack categories. Any failure in this suite blocks releases and pull request merges in CI.

1. **Unauthorized Access:** Missing or malformed credentials return `401 unauthenticated`; cross-owner resource lookups return `404 not_found` (preventing resource enumeration); user access tokens presented to admin routes strictly return `403 forbidden`; WebAuthn passkey assertion verification rejects cloned authenticators (monotonic counter checks); destructive admin operations strictly require step-up re-authentication within a 5-minute window (`SEC-018`, `SEC-004`).
2. **Expired Tokens & Replay Defense:** Relays reject expired allocation tokens (`RejectCode::TokenExpired`) and reject duplicate token `jti`s via in-memory replay cache; admin challenges and sessions expire bounded by TTL; expired invitations return `400 invitation_invalid` (`SEC-004`, `REL-001`, `SEC-003`).
3. **Cross-Session & Multi-Tenant Isolation:** Relays enforce default-deny guest admission tables per `(tunnel_id, canonical_ip)` (`SEC-009`); guests admitted to tunnel A cannot forward traffic to tunnel B; control-plane routes isolate tenants; relays access only their own node state.
4. **Destination Validation & SSRF Defense:** The control plane rejects local destinations in requests (`INV-1`); relays open no client-directed outbound connections (`SEC-005`); engines dial only locally configured targets and refuse unknown or disabled mappings (`0x01`), even under fuzzed frames (`SEC-006`).
5. **Port Exposure & Attack Surface Minimization:** Only explicitly enabled mappings allocate public ports on relays (`GAME-008`); stopped tunnels immediately release ports and admissions (`INV-6`, `INV-7`); mappings strictly conform to game profile specifications (`INV-2`).
6. **Rate Limiting & Abuse Defense:** Sensitive endpoints (`POST /v1/invitations/claim`, `POST /v1/auth/refresh`, admin challenge) return `429 rate_limited` beyond configured limits; memory state is bounded (`max_tracked_keys`) preventing IP churn exhaustion (`SEC-008`).
7. **Secret Leakage & Data Minimization:** Secrets (tokens, invitation secrets, recovery codes) never appear in logs, error responses, or `audit_events` (`SEC-002`, `INV-5`); guest IPs are masked (`/24` IPv4, `/48` IPv6) per `PRIV-002`; database DDL contains 0 email columns (`PRIV-006`) and 0 local target columns (`INV-1`).
8. **Resource Exhaustion & Backpressure:** Relays enforce global memory ceilings (`GlobalRelayMemoryBudget`, 16 MiB) and per-session flow limits (64 TCP / 64 UDP), applying backpressure drops without panicking or leaking memory (`PERF-003`, `NET-006`); request body sizes are bounded pre-parse (`MAX_REQUEST_BODY_BYTES = 16 KiB`); account quotas return `403 quota_exceeded` (`SEC-015`).

---

## 2. Requirements & Verification Target Matrix

| Category ID | Category Name | Requirements | Verification Target |
|---|---|---|---|
| `SEC-CAT-01` | Unauthorized Access | `SEC-018`, `SEC-004`, `SEC-009` | `gtt-control-plane::admin_api_unit`, `gtt-control-plane::policy_unit`, `gtt-relay::relay_skeleton` |
| `SEC-CAT-02` | Expired Tokens | `SEC-004`, `REL-001`, `SEC-003` | `gtt-relay::relay_skeleton`, `gtt-control-plane::limiter_audit_relay_unit`, `gtt-control-plane::admin_api_unit` |
| `SEC-CAT-03` | Cross-Session Isolation | `SEC-009` | `gtt-relay::admission_enforcement`, `gtt-control-plane::policy_unit`, `gtt-engine::network_impairment` |
| `SEC-CAT-04` | Destination Validation | `SEC-005`, `SEC-006`, `INV-1` | `gtt-control-plane::invariants_unit`, `gtt-relay::relay_skeleton`, `gtt-engine::fuzz_relay_inputs` |
| `SEC-CAT-05` | Port Exposure | `INV-6`, `GAME-008`, `INV-2` | `gtt-control-plane::tunnels_routes_unit`, `gtt-control-plane::invariants_unit` |
| `SEC-CAT-06` | Rate Limiting | `SEC-008` | `gtt-control-plane::limiter_audit_relay_unit` |
| `SEC-CAT-07` | Secret Leakage | `SEC-002`, `INV-5`, `PRIV-002`, `PRIV-006` | `gtt-control-plane::limiter_audit_relay_unit`, `gtt-control-plane::admin_api_unit`, migrations DDL inspection |
| `SEC-CAT-08` | Resource Exhaustion | `SEC-015`, `PERF-003`, `NET-006` | `gtt-relay::load_capacity`, `gtt-control-plane::tunnels_routes_unit` |

---

## 3. Running the Suite

### Automated Test Runner
```bash
python tests/security/run_security_tests.py
```

### Targeted Cargo Invocations
```bash
# Admin authentication, clone detection, step-up enforcement
cargo test -p gtt-control-plane --test admin_api_unit

# Policy isolation & indistinguishability
cargo test -p gtt-control-plane --test policy_unit

# Local-target rejection and audit metadata invariants
cargo test -p gtt-control-plane --test invariants_unit

# Rate limiting and log redaction
cargo test -p gtt-control-plane --test limiter_audit_relay_unit

# Relay token validation and outbound connection prohibition
cargo test -p gtt-relay --test relay_skeleton

# Guest admission default-deny and cross-tunnel isolation
cargo test -p gtt-relay --test admission_enforcement

# Relay capacity and memory backpressure saturation
cargo test -p gtt-relay --test load_capacity

# Engine local-target immutability under fuzzing
cargo test -p gtt-engine --test fuzz_relay_inputs
```
