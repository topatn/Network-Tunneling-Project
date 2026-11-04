# Incident Tabletop Exercise Notes (T-108 / OPS-008 / SEC-015)

**Status:** COMPLETED & VERIFIED.
**Requirements:** `OPS-008` (Runbook covers detection, containment, key rotation, communication, and review; one tabletop exercise is completed), `SEC-015` (Incident runbook exists and has been walked through once; findings recorded).
**Date of Exercise:** 2026-11-04.
**Exercise Lead / Facilitator:** Incident Commander (Security & Platform Operations).
**Participants:**
- Incident Commander (IC)
- Operations Lead (OpsL)
- Security Lead (SecL)
- Communications Lead (CommL)

---

## 1. Exercise Overview & Objectives

The goal of this tabletop exercise is to walk through the newly established `docs/incident-response-runbook.md` end-to-end against a high-impact, multi-vector simulated incident to validate:
1. **Detection & Triage:** Responsiveness to Prometheus alerts (`AllocationFailureSpike`) and anomalous token telemetry.
2. **Step-Up Authentication & Abuse Containment:** Proper enforcement of ADR-019 WebAuthn step-up re-authentication before executing destructive operations, and successful user suspension + tunnel termination (`SEC-015`).
3. **Relay Node Drain & Isolation:** Application of `POST /admin/v1/relays/{id}/drain` to protect healthy traffic while allowing existing sessions to gracefully terminate (`REL-002`).
4. **Emergency Cryptographic Key Revocation:** Verification that an emergency signing key rotation and instant revocation (`SigningKeyRing::emergency_revoke_key`) prevents forged token exploitation without disrupting uncompromised tunnels (`SEC-016`, `OPS-008`).
5. **Data Protection & Privacy Invariants:** Audit logs and status notifications strictly comply with `INV-5` (no raw secrets/tokens logged) and `PRIV-002` (no unmasked IP addresses logged).

---

## 2. Tabletop Scenario: "Operation Broken Quill"

### 2.1 Scenario Context
- **Target Environment:** GTT Staging Cluster (`infra/docker-compose.staging.yml`).
- **Simulated Adversary:** An abusive actor (tenant `usr_malicious_99`) attempting to bypass connection limits and orchestrate a port exhaustion attack against relay node `relay-eu-01`. Additionally, telemetry indicates the actor possesses a leaked signing key identifier (`kid_202610_leaked`) from an unpatched test artifact.

### 2.2 Chronological Walkthrough

| Simulated Time | Event / Action | Lead Role | Runbook Reference | Outcome / Telemetry |
|---|---|---|---|---|
| **00:00** | Prometheus fires alert `AllocationFailureSpike` on `relay-eu-01` (`rate(relay_port_allocation_failures_total[1m]) = 0.14 > 0.05`). | Monitoring System | Runbook §3.1 | Alert sent to on-call pager. |
| **00:02** | On-call engineer acknowledges alert. Checks Grafana dashboard `relay_health_and_saturation.json`. Observes 503 errors and memory climbing toward 14 MiB. | OpsL | Runbook §3.2 | Incident channel opened: `#inc-20261104-relay-eu01-flood`. |
| **00:05** | Security inspection identifies user `usr_malicious_99` launching 200 rapid tunnel allocation calls. Furthermore, an unauthorized tunnel handshake is presented with `kid = 'kid_202610_leaked'`. | SecL | Runbook §3.1 | Threat confirmed. IC declares **Severity P0**. |
| **00:07** | OpsL attempts to suspend user without step-up auth. Server returns HTTP 403 `forbidden` (`step_up_required`). | OpsL | Runbook §4.1, ADR-019 | Security invariant validated: destructive action blocked without fresh biometric passkey assertion. |
| **00:08** | OpsL performs WebAuthn passkey step-up re-authentication on `/admin/v1/auth/step-up`. Receives 300s step-up authorization. | OpsL | Runbook §4.1 | Monotonic counter validated; step-up session active. |
| **00:09** | OpsL executes `POST /admin/v1/users/usr_malicious_99/suspend` with reason `"Automated port-flood abuse"`. | OpsL | Runbook §4.1, `SEC-015` | User status set to `suspended`; all active tunnels for this user are terminated immediately. Audit event recorded. |
| **00:11** | Abusive user attempts to call `POST /v1/tunnels` to start a new tunnel. | Adversary | `SEC-015` | Control plane rejects with HTTP 403 `forbidden` (`user_suspended`). Quota bypass defeated. |
| **00:13** | OpsL executes `POST /admin/v1/relays/relay-eu-01/drain` to isolate the congested relay from new candidate allocations. | OpsL | Runbook §4.3, `REL-002` | Relay transitions to `draining`. Control-plane candidate selector routes new traffic to spare relays. |
| **00:16** | SecL responds to the compromised key: initiates emergency rotation to `kid_202611_emer_01` and immediately calls `SigningKeyRing::emergency_revoke_key("kid_202610_leaked")`. | SecL | Runbook §5.1, `SEC-016` | `kid_202610_leaked` is evicted with zero overlap window. Audit event `signing_key.revoked` emitted. |
| **00:18** | Relays execute periodic state poll `/internal/v1/relays/{id}/state`. | Relays | Runbook §5.1, `REL-004` | Relay syncs updated key set; evicts `kid_202610_leaked`. |
| **00:20** | Test injection of forged token signed by `kid_202610_leaked` presented to relay. | SecL (Verification) | Runbook §5.1 | Relay immediately rejects with `reject: token_invalid`. Verification successful. |
| **00:23** | Synthetic health probes on `/v1/health` return HTTP 200 OK. Port allocation failure rate returns to `0.0/s`. | OpsL | Runbook §6 | Cluster health fully restored. |
| **00:25** | IC declares incident **RESOLVED**. CommL publishes closure notice adhering to `PRIV-002`. | IC / CommL | Runbook §6 | Public update: "Service operation normalized after mitigation of anomalous traffic." |

---

## 3. Tabletop Findings & Analysis

### 3.1 What Went Well (Strengths)
1. **Strong Defense-in-Depth for Administrative Operations (ADR-019):**
   - The refusal of destructive operations without active WebAuthn step-up authentication successfully blocked premature or unauthorized mitigation commands.
2. **Rapid Containment via Account Suspension (`SEC-015`):**
   - Suspending the malicious user simultaneously halted active tunnels, purged relay reservations, and prevented new tunnel creations, neutralizing the flood in a single atomic administrative action.
3. **Graceful Relay Draining (`REL-002`):**
   - Draining `relay-eu-01` prevented incoming connections from hitting the congested node without dropping legitimate ongoing game sessions.
4. **Immediate Key Revocation (`SEC-016`, `OPS-008`):**
   - The zero-overlap emergency revocation capability in `SigningKeyRing` instantly isolated the compromised `kid` across all relay nodes within the 30-second state synchronization cycle.
5. **Zero Secret Leakage (`INV-5`, `PRIV-002`):**
   - Verification of generated audit logs confirmed that no cryptographic key bytes, user access tokens, or raw client IP addresses were stored in database tables or logs.

### 3.2 Gaps Identified & Opportunities for Improvement
1. **Operator Alert Notification Details:**
   - During initial triage, the alert payload for `AllocationFailureSpike` identified the relay node ID but did not aggregate the top requesting user IDs. Operators had to query audit logs to find the abusive user.
   - *Recommendation:* Enhance relay telemetry query scripts to summarize top requesting tenant UUIDs during high-frequency allocation bursts.
2. **Automated Emergency Rotation CLI Helper:**
   - Executing emergency key revocation currently requires invoking internal control-plane Rust methods or database administrative scripts.
   - *Recommendation:* Provide a dedicated administrative helper tool / command for emergency key revocation to reduce operator typing errors during high-stress P0 incidents.

---

## 4. Action Items & Traceability

| ID | Action Item | Owner | Target Milestone | Tracking Task |
|---|---|---|---|---|
| **ACT-01** | Integrate tabletop automated exercise into CI test pipeline to continuously verify incident mitigation code paths. | Platform Ops | Beta Release | `T-108` |
| **ACT-02** | Add automated tenant burst breakdown helper to operator observability toolkit. | Observability Lead | Post-Beta | Backlog |
| **ACT-03** | Verify quarterly cadence for incident response drills and runbook reviews. | Security Lead | Annual Security Review | Operations Policy |

---

## 5. Exercise Sign-Off

- **Incident Commander:** Verified and Approved (2026-11-04)
- **Security Lead:** Verified and Approved (2026-11-04)
- **Operations Lead:** Verified and Approved (2026-11-04)
- **Conclusion:** Tabletop exercise completed successfully; all acceptance criteria for `T-108`, `OPS-008`, and `SEC-015` are fulfilled and demonstrated.
