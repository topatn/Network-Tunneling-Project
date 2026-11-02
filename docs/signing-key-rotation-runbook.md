# Signing-Key Management and Rotation Runbook (T-050)

**Status:** DRAFT v0.1 (`T-050`, `SEC-016`, `SEC-004`).
**Authoritative code:** `crates/protocol/src/token.rs` and `crates/control-plane/src/keys.rs`.
**Last updated:** 2026-10-19

> **Verification status (`AGENTS.md` §6.5 & §8):** The key-ring state machine, `kid` header routing, overlap-window verification, emergency key revocation, and post-overlap rejection are verified by unit tests in `crates/protocol/tests/protocol.rs` and `crates/relay/tests/relay_skeleton.rs`. The staging rotation exercise (`T-050` verification) has **not** yet been run because no staging environment exists (`T-104`). Commands in §4 are therefore labeled **illustrative** until executed against staging.

---

## 1. Architecture and Key Lifecycle (`contracts.md` §§2, 7)

Allocation tokens (`contracts.md` §2) authorize one engine tunnel session on one specific relay (`aud = relay_id`, `sub = tunnel_id`, `aid = allocation_id`, `jti = token_uuid`, `iat`, `exp`, `mappings`) and carry a protected header with a key identifier (`kid`).

Each signing key in `SigningKeyRing` (`crates/protocol/src/token.rs`) moves through three states:

| State | Signs new tokens? | Exported to relays for verification? | Transition condition |
|---|---|---|---|
| `ActiveSigning` | **Yes** (sole active signer) | **Yes** (`valid_until_epoch_s: None`) | Promoted on initialization or `SigningKeyRing::rotate` |
| `VerifyOnlyOverlap` | **No** | **Yes** (`valid_until_epoch_s: Some(t)`) | Demoted when a newer key is promoted via `SigningKeyRing::rotate` |
| Expired / Pruned | **No** | **No** (rejected with `reject: token_invalid`) | `now_epoch_s >= valid_until_epoch_s` or `emergency_revoke_key(kid)` |

### Overlap-Window Sizing Invariant (`RotationPolicy`)

To guarantee that no valid in-flight allocation token is rejected during a routine rotation (`SEC-016`), `RotationPolicy::new` (`crates/control-plane/src/keys.rs`) enforces:

$$\text{overlap\_window\_secs} \ge \text{max\_token\_ttl\_secs} + \text{safety\_margin\_secs}$$

where `safety_margin_secs` ($\ge 30\text{ s}$, default $120\text{ s}$) covers the relay state-poll interval (`GET /internal/v1/relays/{relay_id}/state`, `contracts.md` §7) plus maximum NTP clock skew between the control plane and relay nodes.

* Existing QUIC sessions already authenticated at a relay are **unaffected** by key rotation because token verification occurs on `hello` and `token_refresh` (`contracts.md` §10).
* Engines that received an allocation token signed with `kid_old` immediately before rotation can still complete `hello` on the relay throughout the overlap window because `kid_old` remains in the relay's `RelayKeySet` until `valid_until_epoch_s`.
* Once `now_epoch_s >= valid_until_epoch_s`, `kid_old` is automatically rejected by `RelayKeySet::find_valid_key` (`RejectCode::TokenInvalid`) and pruned from the control-plane key ring via `SigningKeyRing::prune_expired`.

---

## 2. Routine Zero-Downtime Rotation Procedure

1. **Pre-flight checks:**
   * Confirm all healthy relays are polling `/internal/v1/relays/{relay_id}/state` with no stale cursor lag (`OPS-002`).
   * Confirm the new key identifier (`kid_new`, matching `^[A-Za-z0-9_-]{1,64}$`) is distinct from all keys currently in the ring.
   * Confirm 256-bit (32-byte) key material is generated from the OS CSPRNG and never written to logs, repository files, or `audit_events` metadata (`PRIV-002`, `INV-5`).
2. **Execute rotation (`SigningKeyRing::rotate`):**
   * Step-up admin authentication is required before invoking the rotation action (`ADR-019`, `SEC-018`).
   * Call `SigningKeyRing::rotate(kid_new, material_new, now_epoch_s, overlap_window_secs)`.
   * Record an `audit_events` row (`actor_type = 'admin'`, `action = 'signing_key.rotated'`) containing only allow-listed metadata (`kid_new`, `retired_kid`, `overlap_window_secs` — **never** key bytes, `INV-5`).
3. **Verify relay distribution during overlap:**
   * Wait one relay state-poll interval.
   * Confirm relays report both `kid_new` (active) and `kid_old` (verify-only until `valid_until_epoch_s`).
   * Confirm connected tunnels remain `Active` without dropped QUIC sessions.
4. **Post-overlap verification:**
   * After `overlap_window_secs` has elapsed, confirm `SigningKeyRing::export_verification_keys` excludes `kid_old` and that a synthetic `hello` carrying a token signed by `kid_old` is rejected with `reject: token_invalid`.

---

## 3. Emergency Key-Compromise Procedure

If a signing key (`kid_compromised`) is suspected of compromise (`OPS-008`):

1. **Rotate immediately** to a fresh CSPRNG key `kid_emergency` via `SigningKeyRing::rotate`.
2. **Revoke the compromised key without overlap** by calling `SigningKeyRing::emergency_revoke_key(kid_compromised)`.
   * This immediately removes `kid_compromised` from `SigningKeyRing::export_verification_keys`.
3. **Force relay state convergence:**
   * On the next `/internal/v1/relays/{relay_id}/state` poll, relays drop `kid_compromised` from their `RelayKeySet` and reject any `hello` or `token_refresh` signed by `kid_compromised` with `reject: token_invalid`.
   * Note: existing sessions that had already authenticated before the compromise window may be terminated via the admin tunnel-termination route (`contracts.md` §9) if the compromise window includes their session start.

---

## 4. Staging Rotation Exercise Checklist (`T-050` Verification — Illustrative)

When the staging environment (`T-104`) is available, execute and record the following steps in `tasks.md` under `T-050`:

1. Start an active tunnel in staging and confirm traffic flows (`Active`).
2. Issue an allocation token $T_1$ under `kid_1` (do not use it yet).
3. Rotate the control-plane key ring to `kid_2` with `overlap_window_secs = 300`.
4. Confirm the already-connected tunnel stays `Active` with 0 packet loss.
5. Within the 300 s overlap window, present $T_1$ (`kid_1`) to the staging relay and confirm `hello_ack` succeeds.
6. Advance past the 300 s overlap window, issue a fresh token with `kid_1`'s material and unexpired `exp`, present it to the staging relay, and confirm the relay returns `reject: token_invalid`.
