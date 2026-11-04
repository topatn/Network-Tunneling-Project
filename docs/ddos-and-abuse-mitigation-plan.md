# DDoS and Abuse Mitigation Plan (T-107 / OPS-005)

**Status:** APPROVED & AUTHORITATIVE.
**Authoritative Requirement:** `OPS-005` (A DDoS and abuse mitigation plan is documented, naming provider-level and application-level controls and response steps; provider capabilities verified from primary documentation).
**Linked Requirements:** `SEC-008` (Rate Limiting), `SEC-009` (Isolation), `SEC-015` (Abuse Controls), `REL-002` (Relay Failover & Draining), `PERF-003` / `PERF-005` (Relay Capacity & Memory Ceilings), `PERF-006` (Egress Cost Control).
**Date:** 2026-11-02.

---

## 1. Threat Landscape & Attack Vectors

Because game server hosting exposes public network listeners, the architecture faces several distinct categories of denial-of-service and abuse vectors:

1. **Volumetric Network-Layer Floods (Layer 3/4):**
   - High-bandwidth UDP reflection/amplification (DNS, NTP, CLDAP, Memcached) targeting relay public game ports.
   - TCP SYN floods targeting control-plane HTTPS endpoints or relay TCP listeners.
   - ICMP and fragmentation floods designed to saturate datacenter upstream transit links.
2. **Game-Protocol Application Floods (Layer 4/7):**
   - Fake handshake floods (e.g. Minecraft handshake packets, RakNet open-connection packets, Source `A2S_INFO` queries) designed to exhaust relay CPU, connection tracking tables, or memory buffers.
   - Large UDP datagram floods attempting to induce memory bloat or out-of-order reassembly buffers.
3. **Endpoint Scanning & Brute-Force Enumeration:**
   - Internet-wide port scanning (Shodan, Censys, masscan) probing reserved ports.
   - Brute-force guessing attacks against invitation secrets (`POST /v1/invitations/claim`).
   - Credential stuffing against authentication endpoints (`POST /v1/auth/exchange`, `POST /v1/auth/refresh`).
4. **Tenant Hoarding & Resource Exhaustion (Internal Abuse):**
   - Malicious or runaway users spawning hundreds of concurrent tunnels to monopolize relay ports and RAM.
   - Single-session flow exhaustion opening thousands of idle TCP connections or UDP streams.
   - Financial exhaustion via asymmetric egress data generation (`PERF-006`).

---

## 2. Provider-Level Protections & Primary Source Verification

Operating public relays and control planes requires upstream infrastructure provider DDoS protection. The capabilities of our primary hosting provider candidates have been verified against their official technical documentation:

### 2.1 OVHcloud (Recommended for Game Relays)
*Primary Documentation Sources:*
- OVHcloud Technical Documentation: *Anti-DDoS Infrastructure and VAC System* (`https://help.ovhcloud.com/csm/en-dedicated-servers-anti-ddos?id=kb_article_view&sysparm_article=KB0043847`)
- OVHcloud Network Security: *Anti-DDoS Game Protection Algorithms* (`https://www.ovhcloud.com/en/security/anti-ddos/anti-ddos-game/`)

*Verified Capabilities:*
- **Multi-Terabit Edge Scrubbing (VAC):** Every OVHcloud server includes automatic, always-on hardware mitigation capable of absorbing > 17 Tbps across global PoPs without traffic rerouting latency penalties.
- **Stateful Game-Specific Filtering (Anti-DDoS Game):**
  - Specifically inspects inbound UDP packets for known game protocols, including **Minecraft Java (TCP)**, **Minecraft Bedrock (UDP / RakNet)**, and **Valve Source Engine (L4D2/L4D UDP query and game packets)**.
  - Two-way deep packet inspection (DPI) verifies valid client protocol handshakes before forwarding packets to the server socket.
  - Drops reflection/amplification attacks and malformed UDP payloads at line rate, distinguishing legitimate players from botnet UDP floods.
- **Cost Protection:** Unlimited DDoS mitigation is included by default with no overage charges for absorbed volumetric attacks.

### 2.2 Hetzner Cloud & Dedicated (Alternative Relay Option)
*Primary Documentation Sources:*
- Hetzner Online Docs: *DDoS Protection & Scrubbing Center* (`https://docs.hetzner.com/general/others/ddos-protection/`)
- Hetzner Cloud Documentation: *Traffic Limits and Server Network Bandwidth* (`https://docs.hetzner.com/cloud/servers/overview#traffic`)

*Verified Capabilities:*
- **Automated Hardware Scrubbing:** Implements an automated perimeter scrubbing center based on Arbor Networks (NETSCOUT) and Juniper hardware.
- **Traffic Signatures Mitigated:** Automatically detects and mitigates SYN floods, DNS/NTP/SNMP reflection attacks, ICMP floods, and teardrop attacks.
- **Limitations:** Filtering is primarily Layer 3 and Layer 4 volume-based; does not perform application-layer protocol validation for custom game payloads. L7 floods or sophisticated low-volume game packet floods require application-level controls (see §3).

### 2.3 Cloudflare (Control-Plane HTTP API & Edge Protection)
*Primary Documentation Sources:*
- Cloudflare Reference Architecture: *DDoS Protection Architecture* (`https://developers.cloudflare.com/ddos-protection/reference/architecture/`)
- Cloudflare WAF & Rate Limiting Documentation: *Rate Limiting Rules* (`https://developers.cloudflare.com/waf/rate-limiting-rules/`)

*Verified Capabilities:*
- **Anycast Global Network (330+ Cities):** Proxies all inbound HTTPS traffic for `api.tunnel.example.com` and `admin.tunnel.example.com`.
- **Autonomous L7 Mitigation:** Analyzes request rates, TLS fingerprints, IP reputations, and behavioral anomalies, dropping HTTP volumetric floods at the edge before hitting origin servers.
- **Edge Rate Limiting:** Enforces edge token buckets on `/v1/invitations/claim` and `/v1/auth/*` before traffic reaches `gtt-control-plane`.

---

## 3. Application-Level Architectural Controls

Regardless of upstream provider scrubbing, the application software stack enforces multi-layered defense-in-depth:

```
                  [ Internet / Attackers / Guests ]
                                 │
                                 ▼
         ┌───────────────────────────────────────────────┐
         │ 1. Provider Edge Scrubbing (OVH VAC / Cloudflare)│
         └───────────────────────┬───────────────────────┘
                                 │ Cleaned Traffic
                                 ▼
         ┌───────────────────────────────────────────────┐
         │ 2. Relay Ingress Socket & Admission Enforcer  │
         │    • Default-Deny Guest Table (canonical IP)  │
         │    • Unadmitted IPs dropped immediately       │
         └───────────────────────┬───────────────────────┘
                                 │ Admitted Guests
                                 ▼
         ┌───────────────────────────────────────────────┐
         │ 3. Relay Capacity & Memory Supervisor         │
         │    • Max 250 Allocations per Node             │
         │    • Max 64 TCP / 64 UDP Flows per Session   │
         │    • Global Memory Budget (16 MiB) Drop Gate  │
         │    • Bandwidth Token Bucket (2 MiB/s / sess)  │
         └───────────────────────┬───────────────────────┘
                                 │ Bound Data Path
                                 ▼
         ┌───────────────────────────────────────────────┐
         │ 4. Host Engine Target Policy Enforcer (SEC-006)│
         │    • Localhost / Loopback Dials Only (INV-1)  │
         │    • Disabled Mapping Refusal (0x01)          │
         └───────────────────────────────────────────────┘
```

### 3.1 Default-Deny Guest Admission (`crates/relay/src/admissions.rs`, `SEC-009`)
- Public relay game ports do not accept open public connections.
- Every inbound connection/datagram is checked against the in-memory `RelayAdmissionEnforcer` table keyed by `(tunnel_id, canonical_guest_ip)`.
- Packets from unadmitted IPs (scanners, random internet probing, unauthorized players) are dropped immediately at socket ingest with zero packet queuing, zero heap allocations, and zero state persistence.

### 3.2 Strict Relay Resource & Backpressure Ceilings (`crates/relay/src/limits.rs`, `PERF-003`, `NET-006`)
- **Max Allocations:** Bounded to 250 concurrent active game servers per relay instance (`DEFAULT_MAX_ALLOCATIONS = 250`). Request 251 is rejected with `MaxAllocationsReached`.
- **Per-Session Flow Limits:** Maximum 64 concurrent TCP connections and 64 concurrent UDP flows per tunnel (`SessionLimits { max_tcp_flows: 64, max_udp_flows: 64 }`). Excess connection attempts receive immediate RST or datagram drop.
- **Global Memory Ceiling:** `GlobalRelayMemoryBudget` enforces a strict 16 MiB (`16,777,216 bytes`) ceiling across all active sessions. If relay buffers saturate under burst attack, excess packets are dropped with `GlobalMemoryCapReached` rather than causing out-of-memory crashes (`OOM`) or unbounded memory leaks.
- **Per-Session Bandwidth Shaping:** Token bucket rate limiter clamps sustained throughput to 2 MiB/s (16 Mbps) with 256 KiB burst per session, preventing bandwidth exhaustion or abusive file transfers.

### 3.3 Control-Plane Quotas & Rate Limits (`crates/control-plane/src/limiter.rs`, `SEC-008`, `SEC-015`)
- **Account Quotas:** Maximum 3 active tunnels per account (`max_live_tunnels_per_user = 3`). Prevents single-user relay port exhaustion.
- **Sliding-Window Rate Limiting:**
  - `Strict` class (10 req/min): Enforced on `POST /v1/invitations/claim` and `POST /v1/auth/refresh`. Exceeding calls return HTTP `429 rate_limited` with `Retry-After`.
  - `max_tracked_keys = 10,000`: Bounded memory table evicts oldest entries, preventing attacker IP churn from exhausting control plane memory.
- **Pre-Parse Request Bounds:** Payload size bounded to 16 KiB (`MAX_REQUEST_BODY_BYTES`) before serde JSON parsing, preventing CPU exhaustion via deeply nested JSON bombs.

### 3.4 Administrative Emergency Controls (`crates/control-plane/src/routes/admin.rs`, `SEC-015`, `REL-002`)
- **Single-User Account Suspension:** `POST /admin/v1/users/{id}/suspend` atomically revokes user sessions, terminates all running tunnels (`INV-7`), and releases reserved relay ports (`INV-6`) in a single transaction.
- **Single-Tunnel Termination:** `POST /admin/v1/tunnels/{id}/terminate` immediately stops an abused tunnel.
- **Node Draining & Failover:** `POST /admin/v1/relays/{id}/drain` sets a relay node to `draining`. Candidate selector immediately stops routing new allocations to the node (`REL-002`), while existing innocent sessions failover to healthy relays.

---

## 4. Step-by-Step Incident Response Procedures

When an attack or abuse event is detected, operators follow this standardized containment sequence:

```
   ┌───────────────────────────────────────────────────────────────┐
   │ 1. DETECTION & CLASSIFICATION                                 │
   │    • Ingest alerts: High dropped flows, memory cap spikes     │
   │    • Classify: Volumetric L3/L4 vs. Single Abusive Tunnel    │
   └───────────────────────────────┬───────────────────────────────┘
                                   │
         ┌─────────────────────────┴─────────────────────────┐
         ▼                                                   ▼
┌─────────────────────────────────┐       ┌─────────────────────────────────┐
│ Path A: Abusive User / Tunnel   │       │ Path B: Infrastructure Attack   │
├─────────────────────────────────┤       ├─────────────────────────────────┤
│ 1. Identify owner_id via DB     │       │ 1. Engage Provider Scrubbing    │
│ 2. Execute Admin Step-Up        │       │    (OVH VAC Game / Cloudflare)  │
│ 3. Call Admin Suspend Route     │       │ 2. Execute Admin Step-Up        │
│    POST /admin/v1/users/:id/    │       │ 3. Drain Compromised Relay      │
│    suspend                      │       │    POST /admin/v1/relays/:id/   │
│ 4. Verify tunnel termination    │       │    drain                        │
│    and port release             │       │ 4. Monitor Failover to Healthy  │
│                                 │       │    Relay Cluster                │
└─────────────────────────────────┘       └─────────────────────────────────┘
                                   │
                                   ▼
   ┌───────────────────────────────────────────────────────────────┐
   │ 3. POST-INCIDENT VERIFICATION & RECOVERY                      │
   │    • Audit log verification (GET /admin/v1/audit-events)     │
   │    • Verify zero secret leaks (INV-5, SEC-002)                │
   │    • Document incident timeline in post-mortem report         │
   └───────────────────────────────────────────────────────────────┘
```

### Procedure 1: Containment of Single Abusive Tunnel or User
*Trigger:* An alert fires indicating a specific tunnel has exceeded flow caps, triggered guest claim brute force, or received complaints.
1. **Identify Actor:** Query `audit_events` or `usage_records` to identify the `tunnel_id` and corresponding `owner_id`.
2. **Step-Up Re-Authentication:** The administrator logs in via WebAuthn passkey at `POST /admin/v1/auth/login` and requests step-up authorization at `POST /admin/v1/auth/step-up`.
3. **Execute Account Suspension:**
   ```bash
   curl -X POST https://api.tunnel.example.com/admin/v1/users/<USER_ID>/suspend \
     -H "Authorization: Bearer <ADMIN_TOKEN>" \
     -H "Content-Type: application/json" \
     -d '{"reason": "DDoS target or Terms of Service abuse"}'
   ```
4. **Verify Immediate Termination:**
   - The user's active tunnels are transitioned to `stopped` (`INV-7`).
   - The relay releases all public ports on the next state poll (`INV-6`).
   - All active `guest_admissions` and invitation tokens for the user are revoked.

### Procedure 2: Containment of Node Saturation via Relay Draining
*Trigger:* A relay node's bandwidth or memory budget approaches the 16 MiB ceiling due to distributed floods targeting multiple ports on a single host.
1. **Engage Provider Game Anti-DDoS:** On the OVHcloud control panel or Hetzner console, verify hardware mitigation has engaged and enable permanent Game DPI filtering for the node's IP.
2. **Drain Relay from Candidate Selection:**
   Issue an administrative drain command to redirect incoming server provisioning to other cluster nodes:
   ```bash
   curl -X POST https://api.tunnel.example.com/admin/v1/relays/<RELAY_ID>/drain \
     -H "Authorization: Bearer <ADMIN_TOKEN>"
   ```
3. **Automatic Failover:**
   - The candidate allocator (`select_candidate_relays`) skips the draining node (`REL-002`).
   - Existing hosts detect degraded performance or connection drop and automatically trigger `reallocate_tunnel_on_failover`, resuming sessions on alternative healthy relays with clean public ports.

### Procedure 3: Ingress Rate Limit & Edge Hardening (Control Plane)
*Trigger:* HTTP API experiences credential stuffing or registration/invitation claim flooding.
1. **Adjust Dynamic Limiter Thresholds:** Update `RateLimitConfig` in control-plane configuration or adjust Cloudflare Edge Rate Limiting rules to reduce burst tolerance from 10 req/min down to 3 req/min for `/v1/invitations/claim`.
2. **Review Dropped Metrics:** Inspect Prometheus metrics `http_requests_total{status="429"}` to confirm malicious sources are clamped.

---

## 5. Verification & Compliance Checklist

- [x] Primary documentation from infrastructure providers reviewed and cited (OVHcloud Anti-DDoS Game, Hetzner DDoS Center, Cloudflare Anycast DDoS).
- [x] Application-level controls implemented in code:
  - `RelayAdmissionEnforcer` default-deny (`crates/relay/src/admissions.rs`, `SEC-009`).
  - `GlobalRelayMemoryBudget` 16 MiB memory cap & flow caps (`crates/relay/src/limits.rs`, `PERF-003`).
  - Sliding-window rate limiting & account quotas (`crates/control-plane/src/limiter.rs`, `SEC-008`, `SEC-015`).
  - Admin suspension, tunnel termination, and relay draining (`crates/control-plane/src/routes/admin.rs`, `SEC-015`, `REL-002`).
- [x] Step-by-step incident response procedures documented with exact API commands.
- [x] Verified by automated tests in `tests/security/run_security_tests.py` (`SEC-CAT-06`, `SEC-CAT-08`).
