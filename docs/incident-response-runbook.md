# Incident Response Runbook (T-108 / OPS-008 / SEC-015)

**Status:** APPROVED & AUTHORITATIVE.
**Authoritative Requirements:** `OPS-008` (Incident response procedure covering detection, containment, key rotation, communication, and review; one tabletop exercise completed), `SEC-015` (Abuse controls: quotas, suspension, tunnel termination, and documented incident procedure).
**Linked References:** `architecture.md` §2–§5, `docs/signing-key-rotation-runbook.md`, `docs/ddos-and-abuse-mitigation-plan.md`, `contracts.md` §9, `schema.md`.
**Date:** 2026-11-04.

---

## 1. Incident Management Framework

This runbook establishes standard operating procedures for detecting, classifying, containing, and resolving security incidents, infrastructure failures, abuse attacks, and cryptographic compromises across the Game Tunneling Tool (GTT) platform.

### 1.1 Incident Response Team Roles

| Role | Responsibility | Authority |
|---|---|---|
| **Incident Commander (IC)** | Coordinates incident lifecycle, assigns tasks, declares severity, approves public communications. | Overall incident lead; decides escalations and resolution. |
| **Operations Lead (OpsL)** | Executes infrastructure controls: relay draining, failover, slot rollback, database health checks. | Direct modification of staging/production cluster state. |
| **Security Lead (SecL)** | Investigates breach scope, executes emergency key revocations, coordinates credential rotation. | Access to security telemetry, key revocation, and admin audit logs. |
| **Communications Lead (CommL)** | Manages internal status updates, external operator communications, and public status notifications. | Drafts and publishes status notices per data privacy invariants (`PRIV-002`). |

---

## 2. Severity Classification Matrix

| Level | Definition | Response SLA | Examples | Escalation |
|---|---|---|---|---|
| **P0 - Critical** | Catastrophic platform outage, signing key compromise, admin account takeover, or active data breach. | Immediate ($\le 15\text{m}$) | Active private key leak, widespread relay drop, unauthorized database write. | Page IC, SecL, OpsL, Executive Leadership. |
| **P1 - High** | Degraded performance, single relay node failure, regional capacity saturation, or active abusive flooding. | Fast ($\le 30\text{m}$) | `AllocationFailureSpike` on relay, DDoS on edge ports, user quota bypass attempt. | Page IC and on-call Ops Lead. |
| **P2 - Medium** | Non-critical component degradation, background cleanup task failures, or isolated user reports. | Standard ($\le 2\text{h}$) | Retention sweep lag, non-critical metric scrape failures, single tunnel restart error. | Ticket assigned to on-call engineer during business hours. |
| **P3 - Low** | Minor cosmetic anomalies, benign log warnings, or non-impacting documentation discrepancies. | Routine ($\le 24\text{h}$) | Outdated runbook URL, benign CLI warning, non-critical telemetry gap. | Standard backlog refinement. |

---

## 3. Phase 1: Detection, Alerting & Triage

### 3.1 Telemetry and Alert Sources
Incidents are detected through automated observability rules and telemetry channels:
1. **Prometheus Alert Rules (`infra/monitoring/prometheus/alert_rules.yml`):**
   - `RelayUnhealthy`: Relay heartbeat missing $> 30\text{s}$ or control plane unreachable.
   - `AllocationFailureSpike`: Port allocation failure rate exceeds $0.05\text{/s}$ over 1 minute.
   - `RelayMemoryBudgetNearExhaustion`: Buffer memory exceeds $90\%$ of 16 MiB ceiling (`PERF-003`).
   - `HighHttpErrorRate`: Control plane 5xx error rate $> 5\%$.
2. **Security Telemetry & Audit Logs (`audit_events`):**
   - Monotonic WebAuthn counter rollbacks (indicating cloned passkeys per ADR-019).
   - Rapid authentication failures or user token presentation on admin routes (HTTP 403 `forbidden`).
   - Sudden spike in guest admission rejections (`SEC-009`).
3. **External Abuse Reports:**
   - Provider abuse notifications (OVHcloud VAC / Hetzner automated reports).
   - Player or server host incident reports.

### 3.2 Triage & Declaration Procedure
1. Incident responder acknowledges the alert within SLA.
2. Responder opens an incident communication channel (`#inc-<date>-<brief-slug>`).
3. Responder assesses customer impact, system telemetry, and declares severity (P0–P3).
4. IC designates roles (OpsL, SecL, CommL).

---

## 4. Phase 2: Containment & Abuse Mitigation

All containment actions follow the principle of least disruption: isolate offending entities and mitigate malicious traffic while preserving unaffected legitimate game tunnels (`REL-001`).

### 4.1 Abusive User Suspension (`SEC-015`)
When an account is identified as abusive, participating in DDoS attacks, or attempting token forgery:

1. **Step-Up Authentication:** Administrator verifies identity via WebAuthn passkey on `/admin/v1/auth/step-up`. Destructive operations require an active step-up window ($\le 300\text{s}$, ADR-019).
2. **Execute Suspension:**
   ```bash
   curl -X POST \
     -H "Authorization: Bearer $ADMIN_TOKEN" \
     -H "Content-Type: application/json" \
     -d '{"reason": "DDoS port flooding abuse detected"}' \
     http://control-plane.internal:8080/admin/v1/users/$USER_ID/suspend
   ```
3. **Automated Enforcement:**
   - Account status transitions to `suspended`.
   - All active tunnels owned by the user are immediately terminated in the database and relay reservations dropped.
   - Active user sessions (`auth_sessions`) are invalidated.
   - Subsequent calls to create or start tunnels return HTTP 403 `forbidden` (`user_suspended`).
   - Append-only audit event recorded: `action = 'admin.user_suspended'`.

### 4.2 Rogue Tunnel Termination (`SEC-015`)
To terminate a specific abusive tunnel without suspending the parent user account:
```bash
curl -X POST \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"reason": "Exceeded bandwidth quota / malicious payload"}' \
  http://control-plane.internal:8080/admin/v1/tunnels/$TUNNEL_ID/terminate
```
- Tunnel status transitions to `terminated`.
- Relay allocations are freed immediately.
- Re-connection attempts with existing tokens are rejected with `reject: token_invalid`.

### 4.3 Relay Node Isolation and Draining (`REL-002`)
If a relay node experiences extreme memory saturation, network flapping, or hardware failure:
```bash
curl -X POST \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  http://control-plane.internal:8080/admin/v1/relays/$RELAY_ID/drain
```
- Relay status transitions to `draining`.
- Control plane skips this node for all new allocation requests (`REL-002`).
- Existing tunnels remain connected until players disconnect or game server closes.
- Once existing sessions complete, node can be safely rebooted or replaced.

### 4.4 Edge Rate Limiting & Provider Mitigation
Refer to `docs/ddos-and-abuse-mitigation-plan.md` for L4/L7 DDoS mitigation:
- OVHcloud VAC Game profile automatic packet filtering.
- Sliding-window IP rate limiter on control plane edge (`SEC-008`).

---

## 5. Phase 3: Emergency Key Rotation & Secret Revocation

If allocation token signing keys, admin credentials, or relay secrets are suspected of compromise, execute emergency rotation per `docs/signing-key-rotation-runbook.md` §3.

### 5.1 Emergency Allocation Token Key Revocation (`SEC-016`, `OPS-008`)
1. **Rotate Signing Key:**
   The control plane promotes a fresh 256-bit CSPRNG key `kid_emergency` into the active signer position:
   ```rust
   signing_key_ring.rotate(kid_emergency, new_key_bytes, now_epoch_s, overlap_window_secs);
   ```
2. **Immediate Revocation (Zero Overlap):**
   Unlike routine rotations which allow a 5-minute overlap window, compromised keys are revoked instantly:
   ```rust
   signing_key_ring.emergency_revoke_key(kid_compromised);
   ```
3. **Relay State Convergence:**
   - Healthy relay nodes poll `/internal/v1/relays/{id}/state` every 30 seconds (`contracts.md` §7, `REL-004`).
   - Relays immediately purge `kid_compromised` from `RelayKeySet`.
   - Any synthetic or forged allocation token signed by `kid_compromised` is rejected upon presentation with `reject: token_invalid`.
4. **Audit Trail:**
   Audit record appended with `action = 'signing_key.revoked'`, recording only `kid_compromised` (zero raw key bytes logged per `INV-5`).

### 5.2 Admin Session Invalidation
If an admin session token is exposed or lost:
1. Revoke the session row in `admin_sessions`:
   ```sql
   UPDATE admin_sessions SET revoked_at = NOW() WHERE token_hash = $HASH;
   ```
2. If passkey credentials are compromised, rotate the administrator's WebAuthn credential ID and reset the passkey counter to 0.

---

## 6. Phase 4: Communication & Disclosure

1. **Internal Status Sync:**
   - IC posts 30-minute status summaries in `#incident-announcements`.
   - Update includes: Current Severity, Impacted Services/Regions, Containment Actions Taken, Next ETA.
2. **External / Player Status Page:**
   - CommL publishes public updates on status page (e.g., `status.gtt.io`).
   - Communication must adhere to privacy invariants:
     - **NEVER** disclose player IP addresses, email addresses, or unmasked identifiers (`PRIV-002`).
     - **NEVER** reveal internal cluster hostnames, secret keys, or vulnerability reproduction steps (`INV-5`).
3. **Post-Resolution Communication:**
   - Notify users when service returns to nominal operation.
   - Outline impact duration and high-level cause.

---

## 7. Phase 5: Post-Incident Review (PIR) & Retrospective

Within 48 hours of resolving a P0 or P1 incident, the IC conducts a blameless Post-Incident Review.

### 7.1 PIR Report Structure
Every incident report must include:
1. **Summary & Timeline:** Chronological sequence of events from initial injection/detection to resolution.
2. **Impact Assessment:** Number of affected users, dropped tunnels, degraded relay nodes, and total downtime.
3. **Root Cause Analysis (5-Whys):** Systematic identification of technical and procedural root causes.
4. **Telemetry Analysis:** Relevant Prometheus graphs, alert timings, and audit event excerpts.
5. **Corrective & Preventive Actions:** Measurable tasks assigned to specific owners and tracked in `tasks.md`.

---

## 8. Summary Checklist for On-Call Operators

```text
[ ] 1. Acknowledge alert and open incident channel (#inc-<date>).
[ ] 2. Determine severity (P0-P3); appoint IC, OpsL, SecL.
[ ] 3. Authenticate with WebAuthn passkey; perform step-up re-authentication.
[ ] 4. Containment:
      [ ] If user abuse: POST /admin/v1/users/{id}/suspend
      [ ] If rogue tunnel: POST /admin/v1/tunnels/{id}/terminate
      [ ] If relay failing: POST /admin/v1/relays/{id}/drain
[ ] 5. Key Compromise:
      [ ] Promote kid_emergency via SigningKeyRing::rotate
      [ ] Revoke kid_compromised via SigningKeyRing::emergency_revoke_key
[ ] 6. Verify health probes: GET /v1/health -> 200 OK.
[ ] 7. Publish status updates (observing PRIV-002 and INV-5).
[ ] 8. Schedule PIR within 48 hours and log action items in tasks.md.
```
