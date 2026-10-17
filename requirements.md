# requirements.md — Requirements and Acceptance Criteria

**Status:** DRAFT v0.1.1. Authoritative for requirements and acceptance criteria. Task mapping is authoritative in `tasks.md`; the traceability appendix below is derived from it.
**Last updated:** 2026-10-17 (SRV group added; scope change recorded in `context.md` G-7 and amended NG-1)

## How to read this document

- **Priority:** P0 required for 1.0; P1 should; P2 could; P3 deliberately deferred (listed so it is not accidentally built).
- **Release target:** Prototype, Alpha, Beta, 1.0, Post-1.0. These are planning labels, not dates.
- **Tags:** `[E-n]` marks a dependency on experiment *n* from `research.md`; `[OPEN]` marks an unresolved design decision.
- **No unsupported targets.** Where a numeric target would be a guess (latency, throughput, capacity, timings, limits), the requirement says a baseline must be measured first and where the target will be recorded. This is deliberate: no figure here has been measured.
- **Games:** a game counts as supported only after its L2 (real-client) test passes (`research.md` ADR-011). Terraria, Minecraft Java, Minecraft Bedrock, Left 4 Dead, and Left 4 Dead 2 each have their own requirement (GAME-001 to GAME-005).
- Several requirements depend on the guest source-IP admission mechanism, which is unproven (E-13). If that experiment fails, FR-007 to FR-009 and SEC-007 are revised.
- The **SRV** group (host-local game server provisioning) was added on 2026-10-17. It **extends** rather than replaces FR-010, FR-011, and UX-002: provisioning adds a fourth *source* of status, it does not merge any of the three existing indicators. All `[E-15]`–`[E-19]` items are unverified.
- **SRV-011** adds a *fifth*, opt-in fact: presence **confirmed by the game server**. It never replaces the traffic-based players-seen indicator, and FR-012's rule that traffic is not presence still holds in every other case.

Counts: 110 requirements, 86 tasks.


## Functional requirements

| ID | Requirement | Pri | Rationale | Acceptance criteria | Verification | Depends on | Target |
|---|---|---|---|---|---|---|---|
| **FR-001** | Host can sign in through the system browser and sign out. | P0 | Ownership, quotas and abuse control need identity. | After browser sign-in the app shows a signed-in state; after sign-out the refresh token used before sign-out is rejected with `unauthenticated`. | Integration test; manual | SEC-001, SEC-003 | Alpha |
| **FR-002** | Host can select a game profile from the five initial games. | P0 | Profiles drive ports and validation. | The profile list equals the profiles returned by `GET /v1/profiles` that also exist locally; selecting one loads its mappings. | UI test | GAME-007 | Alpha |
| **FR-003** | Host can set a local target (host, port) per enabled mapping. | P0 | Destinations must be explicit (ADR-003). | Values violating the local-target policy (`contracts.md` §13) are rejected with a message; captured network traffic during configuration contains no local target. | Unit test; integration test with request capture | SEC-006 | Alpha |
| **FR-004** | Host can create a tunnel for a profile. | P0 | Tunnel record precedes allocation. | Valid `POST /v1/tunnels` returns 201 and a `Tunnel`; invalid bodies return `validation_failed` with field details; a replay with the same idempotency key returns the same tunnel. | API test | FR-002, SEC-009 | Alpha |
| **FR-005** | Host can start a tunnel and see the public address per mapping. | P0 | Core value. | After start the UI shows relay host and public port for each enabled mapping and the engine reaches `Active` in the test environment. | Integration test | NET-003, FR-004 | Alpha |
| **FR-006** | Host can stop a tunnel. | P0 | Control over exposure. | After stop the tunnel is `stopped`, the allocation `released`, and connections to the former public ports are refused or time out in the test. | Integration test | FR-005 | Alpha |
| **FR-007** | Host can create invitations with expiry and maximum claims. | P0 | Scoped, expiring guest access. | Secret appears once; list responses never include secrets; expired, revoked, and exhausted secrets all return `invitation_invalid`. | API test | SEC-007 | Alpha |
| **FR-008** | Guest can claim an invitation and receive connection details. [E-13] | P0 | Guests must learn the address and be admitted. | A valid claim returns connect details; in the integration test the relay accepts traffic from the claimed source IP and refuses an unclaimed IP. | Integration test | FR-007, NET-001 | Beta |
| **FR-009** | Host can revoke an invitation or an admission. [E-13] | P0 | Revocable access. | After revocation new guest connections from the revoked source are refused; the time until enforcement is measured and recorded (target set after PERF-007 baseline). | Integration test; measurement | FR-008, PERF-007 | Beta |
| **FR-010** | UI shows tunnel state, local-server reachability, and players seen as separate indicators. | P0 | Prevents one misleading 'connected' light. | A test drives each of the three indicators up and down independently; the display of each changes without affecting the others. | UI test | UX-002 | Alpha |
| **FR-011** | Engine can probe local server reachability. | P0 | Hosts need to know if the game server is listening. | TCP mappings: probe reports reachable for a listening port and unreachable otherwise. UDP mappings: probe uses a profile-defined query where one exists, otherwise reports `unknown`; it never reports `reachable` without a response. | Unit + integration test with real and stopped servers | GAME-007 | Alpha |
| **FR-012** | UI shows players seen from relay traffic. | P1 | Feedback that guests are arriving. | With N scripted guest sources the displayed count equals N; the UI labels it as observed traffic, not confirmed in-game presence. | Integration test | FR-010, NET-001 | Beta |
| **FR-013** | User can export a diagnostics bundle. | P1 | Support without telemetry. | An automated scan of the bundle finds no tokens, secrets, raw guest IPs, or payload bytes. | Security test | PRIV-002 | Beta |
| **FR-014** | Tunnel recovers automatically from a network interruption. | P0 | Home networks drop. | After a simulated outage shorter than the relay grace period, the engine returns to `Active` without user action, the UI shows Reconnecting then Active, TCP guest connections are closed, and UDP flows resume. | Network test | NET-004 | Alpha |
| **FR-015** | UI explains each failure class with a cause and next step. | P1 | Hosts cannot diagnose networks. | For each failure class (relay unavailable, UDP blocked, local server down, token expired, invitation invalid) the UI shows a distinct message and a next action. | UI test | UX-003 | Beta |

## Networking requirements

| ID | Requirement | Pri | Rationale | Acceptance criteria | Verification | Depends on | Target |
|---|---|---|---|---|---|---|---|
| **NET-001** | Relay and engine forward TCP byte streams without altering or losing data. | P0 | Correctness baseline. | A test transfers a known data set in both directions through the tunnel; hashes match; half-close in each direction is honored. | Network test | — | Alpha |
| **NET-002** | Relay and engine forward UDP datagrams with correct flow separation. | P0 | Three of five games are UDP-first. | Two guest sources sending to the same mapping receive only their own replies; datagrams arrive byte-identical when the path does not drop them. | Network test | — | Alpha |
| **NET-003** | Host connectivity requires no inbound ports on the host. | P0 | Product premise. | With all inbound connections to the host blocked by firewall rules, a tunnel starts and carries traffic. | Network test | — | Alpha |
| **NET-004** | Engine resumes a session within the relay grace period. | P0 | Resilience. | A reconnect within the grace period keeps the same public ports; after the period the allocation is released. Period value is set after E-6 data. | Network test | NET-005 | Alpha |
| **NET-005** | Liveness is detected with heartbeats and idle timeouts. | P0 | Stale sessions waste capacity. | A silently dropped peer is detected and the session enters Reconnecting/closed within the configured timeout in the test harness. | Network test | — | Alpha |
| **NET-006** | Per-session limits bound flows, queues, and bandwidth. | P0 | Resource-exhaustion defense. | Exceeding `max_tcp_flows`/`max_udp_flows` or queue limits is refused or dropped and counted; relay memory stays bounded during a flood test (bound set after PERF-005). | Network + security test | PERF-005 | Beta |
| **NET-007** | Oversized or malformed UDP datagrams and invalid protocol messages are dropped and counted without crashing. | P0 | Untrusted input. | A fuzz and malformed-input suite produces no panic or hang; counters increment; sessions of other tunnels are unaffected. | Unit + fuzz + security test | — | Beta |
| **NET-008** | Shutdown closes all sockets and releases resources. | P0 | Avoid leaks and stuck ports. | After stop or crash recovery no listener remains open on the host or relay for that tunnel; verified by port scan in the test. | Integration test | FR-006 | Alpha |
| **NET-009** | A fallback carrier exists when UDP to the relay is blocked. [E-14] | P1 | Restrictive networks. | With UDP to the relay blocked, the tunnel can still reach `Active` via the fallback and the UI indicates reduced mode; behavior and limits are documented. | Network test | — | Beta |
| **NET-010** | IPv4 is supported; IPv6 behavior is documented and tested for the claim/game address mismatch case. [E-13] | P1 | Mismatch can break admission. | IPv4 passes all network tests; for a guest claiming over IPv6 and playing over IPv4 (and vice versa) the documented outcome matches observed behavior. | Network test | FR-008 | Beta |
| **NET-011** | Direct peer-to-peer upgrade is not part of the initial release. | P3 | Deferred (ADR-002). | Release scope check confirms no direct-connect code path is enabled in the 1.0 build. | Release review | — | Post-1.0 |

## Supported-game requirements

| ID | Requirement | Pri | Rationale | Acceptance criteria | Verification | Depends on | Target |
|---|---|---|---|---|---|---|---|
| **GAME-001** | Terraria: direct connect by address works through the tunnel. | P0 | Initial game. | L0, L1, and L2 (`research.md` ADR-011) pass for Terraria; whether UDP is needed (E-1) is resolved and documented. | Game compatibility test; manual L2 | NET-001, GAME-007 | Beta |
| **GAME-002** | Minecraft Java Edition: direct connect works; optional query and RCON mappings default off. | P0 | Initial game. | L0–L2 pass; a connection to the RCON port through the tunnel is impossible unless the host enabled that mapping. | Game compatibility test; security test | NET-001, GAME-008 | Beta |
| **GAME-003** | Minecraft Bedrock Edition: direct connect works over UDP, including non-default ports. Console and mobile players are **out of scope** and cannot use a relay address. | P0 | Initial game. | L0–L2 pass with the default port and with a changed port; IPv6 port behavior (E-2) is documented; `enable-lan-visibility` binding behavior on a non-default port is confirmed; the support scope states in the UI and docs that Xbox, PlayStation and Switch players **cannot** join through a relay-allocated IP:port and why (`research.md` RQ-13.3). | Game compatibility test; manual L2 | NET-002, GAME-007 | Beta |
| **GAME-004** | Left 4 Dead 2: direct connect via the game's `connect` command works. | P0 | Initial game. | L0–L2 pass for direct connect. Lobby and matchmaking joins are reported as supported only if GAME-010 passes. | Game compatibility test; manual L2 | NET-002, GAME-007 | Beta |
| **GAME-005** | Left 4 Dead: direct connect works, with its required protocols confirmed by capture. [E-5] | P0 | Initial game; weakest research evidence. | Required ports and protocols are established by capture and documented; L0–L2 pass. | Capture analysis; game compatibility test | NET-002 | Beta |
| **GAME-006** | The UI and docs state each game's support scope and level. | P0 | No overclaiming. | A game shows as supported only after its L2 test passed in the current release; the scope text matches `research.md`. | Release review; UI test | TEST-005 | 1.0 |
| **GAME-007** | Game profiles are declarative, versioned, and validated at load. | P0 | Modularity. | An invalid profile is rejected at load with a clear error; adding a sixth profile file requires no engine code change in a sample test. | Unit test | — | Alpha |
| **GAME-008** | Optional mappings (query, RCON) are disabled by default. | P0 | Avoid silent exposure. | A newly created tunnel for each profile exposes only the profile's default-enabled mappings. | Unit + integration test | GAME-007 | Alpha |
| **GAME-009** | Custom local server ports are supported. | P0 | Defaults are not guaranteed. | For each game a non-default local port configured in the UI is forwarded correctly in the L0 test. | Integration test | FR-003 | Beta |
| **GAME-010** | Steam lobby/matchmaking joins for Left 4 Dead games are supported only if experiment E-4 passes. | P2 | Unknown feasibility. | If E-4 fails, the docs and UI state direct connect only; if it passes, an L2 test covers the lobby path. | Experiment result; manual L2 | GAME-004 | Post-1.0 |

## Security requirements

| ID | Requirement | Pri | Rationale | Acceptance criteria | Verification | Depends on | Target |
|---|---|---|---|---|---|---|---|
| **SEC-001** | Sign-in uses the system browser with authorization code and PKCE. | P0 | RFC 8252 native-app guidance. | No embedded web view is used for sign-in; redirect URI is checked against the allow-list; a wrong verifier is rejected. | Integration + security test | — | Alpha |
| **SEC-002** | Tokens are stored only in the OS credential store; secrets never appear in files or logs. | P0 | Credential theft. | Scan of settings files, logs, and crash dumps in tests finds no token or secret strings. | Security test | — | Alpha |
| **SEC-003** | Refresh tokens rotate; reuse of a rotated token revokes the session family. | P0 | Stolen-token detection. | Presenting a previously used refresh token returns `unauthenticated` and all tokens in that family stop working. | Integration test | — | Alpha |
| **SEC-004** | Relay validates allocation tokens (signature, audience, expiry, replay). | P0 | Relay trust boundary. | Tokens with a bad signature, wrong audience, expiry in the past, or a reused `jti` are rejected with the matching `reject` code. | Unit + security test | — | Alpha |
| **SEC-005** | Relay never opens connections to destinations chosen by clients. | P0 | SSRF-like abuse (ADR-003). | Code review and tests show the relay has no outbound connect path other than control-plane polling; attempts to supply a target in any message are rejected. | Security test; code review | — | Alpha |
| **SEC-006** | Engine dials only targets from its own configuration, subject to the local-target policy. | P0 | Prevent reaching other local or LAN services. | Unknown or disabled mapping indexes are refused (`0x01`); policy-violating targets cannot be configured; no relay message can change a target. | Unit + security test | — | Alpha |
| **SEC-007** | Invitation secrets have at least 128 bits of entropy, are stored hashed, expire, and are revocable. | P0 | Stolen-invitation defense. | Database inspection shows no plaintext secrets; generation test checks length/entropy source; expiry and revocation are enforced. | Unit + security test | — | Alpha |
| **SEC-008** | Sensitive routes are rate limited. | P0 | Brute force, enumeration. | Claim and refresh endpoints return `rate_limited` beyond the configured limit; limits are set in configuration after load measurement. | Security test | — | Beta |
| **SEC-009** | Users, tunnels, and allocations are isolated. | P0 | Multi-tenant safety. | A user cannot read or change another user's tunnels (`not_found`); a relay cannot read another relay's state; a session cannot forward traffic for another tunnel's mappings. | Security test | — | Alpha |
| **SEC-010** | Client-to-API and client-to-relay channels use authenticated encryption. | P0 | Transport security. | Plaintext HTTP and unauthenticated QUIC are refused; certificate validation failure aborts the connection. | Security test | — | Alpha |
| **SEC-011** | Encryption limits are disclosed. | P0 | Honest security claims. | Docs and in-app text state that the relay can read game traffic in the default guest model and that transport encryption is not end-to-end. | Doc/UI review | PRIV-005 | Alpha |
| **SEC-012** | Normal operation needs no administrator rights. | P0 | Least privilege. | Install, run, update, and uninstall succeed under a standard user account in a clean VM. | Manual VM test | INST-001 | Beta |
| **SEC-013** | Security-relevant actions produce audit events without secrets. | P0 | Abuse review. | Each listed action (sign-in, tunnel start/stop, invitation create/revoke, claim, admin action) creates an event; a leak test finds no tokens, payloads, or raw guest IPs. | Integration + security test | — | Beta |
| **SEC-014** | Application updates are signed and verified; insecure updater options are disabled. | P0 | Supply-chain risk. | A tampered update artifact is rejected; configuration review confirms insecure options are off. | Security test (E-9) | INST-002 | Beta |
| **SEC-015** | Abuse controls exist: quotas, suspension, tunnel termination, and a documented incident procedure. | P0 | Public endpoints invite abuse. | A suspended user's active tunnels are terminated and new starts refused; quota limits return `quota_exceeded`; the incident runbook exists and has been walked through once. | Integration test; tabletop exercise | OPS-008 | Beta |
| **SEC-016** | Signing keys can be rotated without downtime. | P0 | Key compromise response. | In staging a key rotation completes while tunnels remain connected; old keys stop validating after the overlap window. | Staging exercise | — | Beta |
| **SEC-017** | Dependencies are inventoried and scanned in CI. | P1 | Maintenance and vulnerability response. | CI fails on a known-vulnerable dependency above a configured severity; each dependency has a recorded purpose. | CI check | — | Alpha |
| **SEC-018** | Admin operations require separate, stronger authentication and are audited. [OPEN] | P0 | Admin compromise is high impact. | The admin authentication design is documented and reviewed before any admin route ships; admin routes fail without it. | Design review; security test | SEC-013 | Beta |

## Reliability requirements

| ID | Requirement | Pri | Rationale | Acceptance criteria | Verification | Depends on | Target |
|---|---|---|---|---|---|---|---|
| **REL-001** | Existing tunnels survive a control-plane outage until their credentials expire. | P0 | Control plane is not on the data path. | With the control plane stopped, a running tunnel continues to carry traffic; new starts fail with a clear error. | Failure-injection test | — | Beta |
| **REL-002** | An unhealthy relay is detected and its tunnels can be reallocated. | P0 | Relay failure is expected. | When a relay stops reporting health, new allocations avoid it; the documented recovery path moves a tunnel to another relay; address-stability behavior (open issue) is documented. | Failure-injection test | OPS-002 | Beta |
| **REL-003** | Timeouts and retries follow `contracts.md` §4. | P0 | Predictable failures. | Client tests confirm retries on 429/5xx with backoff, no retry without an idempotency key, and bounded attempts. | Unit test | — | Alpha |
| **REL-004** | A relay restart recovers state from the control plane. | P1 | Operational resilience. | After a relay restart, reservations and admissions are restored from the state poll and engines reconnect within the retry policy. | Failure-injection test | — | Beta |

## Performance requirements

| ID | Requirement | Pri | Rationale | Acceptance criteria | Verification | Depends on | Target |
|---|---|---|---|---|---|---|---|
| **PERF-001** | Relay-added latency is measured per game and a target is selected from the baseline. | P0 | No unsupported promises. | Benchmark report with method and environment exists; a target is recorded in `research.md` as an ADR before any latency claim is made. | Benchmark | — | Beta |
| **PERF-002** | Throughput is measured per game and per transport. | P0 | Capacity planning. | Benchmark report exists; targets set from it. | Benchmark | — | Beta |
| **PERF-003** | Engine CPU and memory are measured idle and under load. | P1 | Desktop footprint. | Report exists for idle, one guest, and the maximum configured flows; targets set from it. | Benchmark | — | Beta |
| **PERF-004** | Connection establishment time is measured. | P1 | Perceived speed. | Report of start-to-Active time distribution on a defined network setup; target set from it. | Benchmark | — | Beta |
| **PERF-005** | Relay capacity (concurrent sessions and flows per instance) is measured. | P0 | Cost and limits. | Load-test report identifies saturation behavior; per-relay `max_allocations` is set from it. | Load test | — | Beta |
| **PERF-006** | Bytes per player-minute are measured for each game. [E-10] | P0 | Egress dominates cost. | Measurement report for each of the five games exists and feeds the cost model. | Measurement | — | Beta |
| **PERF-007** | Revocation latency is measured. | P0 | Security of guest access. | Measured time from revoke to enforcement at the relay is recorded; the target is set from the measurement. | Measurement | — | Beta |

## UI/UX requirements

| ID | Requirement | Pri | Rationale | Acceptance criteria | Verification | Depends on | Target |
|---|---|---|---|---|---|---|---|
| **UX-001** | A new host reaches a started tunnel without networking terminology. | P0 | Nontechnical users. | In a usability session with a defined script, participants complete first-tunnel setup; success rate and time are recorded and a target set from the first round. | Usability test | — | Beta |
| **UX-002** | Application status never merges tunnel, local-server, and player status. | P0 | Prevent false assurance. | Design review plus UI test (see FR-010). | UI test; design review | — | Alpha |
| **UX-003** | Errors state a cause and an action. | P1 | Self-service troubleshooting. | Every error code in `contracts.md` §3 and every engine error has a user-facing message reviewed for cause and action. | Review; UI test | — | Beta |
| **UX-004** | All interactive controls are keyboard operable with visible focus. | P0 | Accessibility. | Keyboard-only run through onboarding, start, share, and stop succeeds; focus indicator is visible on every control. | Manual + automated UI test | — | Beta |
| **UX-005** | Text and status colors meet the contrast thresholds in `DESIGN.md`; status is never color-only. | P0 | Accessibility. | Automated contrast check over defined tokens passes; each status also has text or an icon. | Automated check | — | Beta |
| **UX-006** | Stopping a tunnel with observed players requires confirmation. | P1 | Avoid accidental disruption. | Stop with players seen shows a confirmation; without players it does not. | UI test | FR-012 | Beta |
| **UX-007** | Theme support (light/dark) follows the decision in `DESIGN.md`. | P2 | Comfort. | Whichever themes `DESIGN.md` adopts pass UX-005. | Automated check | UX-005 | 1.0 |

## Infrastructure and operations requirements

| ID | Requirement | Pri | Rationale | Acceptance criteria | Verification | Depends on | Target |
|---|---|---|---|---|---|---|---|
| **OPS-001** | Development, staging, and production environments are separate. | P0 | Safe change. | Distinct configuration, credentials, and signing keys per environment; production secrets are not present in staging. | Infrastructure review | — | Beta |
| **OPS-002** | Metrics, structured logs, and alerts cover relay health and saturation. | P0 | Operate the service. | Alerts fire in staging for: relay unhealthy, allocation failure spike, and port-pool or bandwidth saturation. | Staging exercise | — | Beta |
| **OPS-003** | Deployment and rollback procedures exist and are rehearsed. | P0 | Recoverability. | A staged rollback is performed successfully and timed; steps are in the runbook. | Staging exercise | — | Beta |
| **OPS-004** | Relay connection and bandwidth limits are enforced. | P0 | Cost and fairness. | Per-allocation and global caps (values set after PERF-005) throttle or refuse excess and are visible in metrics. | Load test | PERF-005 | Beta |
| **OPS-005** | A DDoS and abuse mitigation plan is documented. | P0 | Public ports are exposed. | Plan names provider-level and application-level controls and the response steps; provider capabilities are verified from their documentation (not yet researched). | Document review | — | Beta |
| **OPS-006** | Relay egress cost is monitored. | P1 | Cost control. | A dashboard shows egress and estimated cost per day; an alert triggers at an operator-set threshold. | Staging exercise | PERF-006 | Beta |
| **OPS-007** | Database backup and restore are tested. | P0 | Data protection. | A restore from backup into a clean database succeeds and the application passes a smoke test. | Restore exercise | — | Beta |
| **OPS-008** | An incident response procedure exists. | P0 | Security and outage events. | Runbook covers detection, containment, key rotation, communication, and review; one tabletop exercise is completed. | Tabletop exercise | — | Beta |

## Privacy requirements

| ID | Requirement | Pri | Rationale | Acceptance criteria | Verification | Depends on | Target |
|---|---|---|---|---|---|---|---|
| **PRIV-001** | The desktop client sends no telemetry by default. | P0 | Privacy by default. | Network capture of a default install during normal use shows traffic only to the API and relay endpoints needed for function. | Network capture test | — | Alpha |
| **PRIV-002** | Logs and diagnostics exclude tokens, payload bytes, and raw guest IPs. | P0 | Data minimization. | Automated redaction tests and a log-scan over a test run find none of these. | Security test | — | Alpha |
| **PRIV-003** | Retention periods are defined and enforced for guest IPs, audit events, usage records, and sessions. | P0 | Limit stored personal data. | Retention values are recorded in this document; a scheduled job test shows expired rows are removed. | Integration test | — | Beta |
| **PRIV-004** | A user can delete their account and associated data. | P0 | User control. | After deletion, the user's rows (users, sessions, tunnels, invitations) are gone; audit entries retain no personal data beyond what policy allows. | Integration test | PRIV-003 | Beta |
| **PRIV-005** | The privacy notice states what relays and the operator can see. | P0 | Honest disclosure. | The notice names relayed game traffic, retained metadata, and retention periods and matches actual behavior. | Document review | SEC-011 | Alpha |
| **PRIV-006** | No email address is stored unless a later decision requires it. | P1 | Data minimization. | Schema review confirms no email column; any change requires an ADR. | Schema review | — | Alpha |

## Host-local game server provisioning requirements (added 2026-10-17)

Scope and non-goals for this group are in `context.md` §5 G-7, §6 NG-1, and §7. Provisioning runs on the host's own PC only. `[E-15]`–`[E-19]` are experiments in `research.md` §8; none has been run, so no numeric or behavioral claim here is verified.

**First cut is Minecraft Java Edition only** (ADR-013, decided 2026-10-17). **Bedrock is excluded by `SRV-003`**: no integrity mechanism for its artifact could be established, so `SRV-002` cannot be met (`research.md` RQ-13.3). Bedrock is deferred, not rejected, and returns if a published checksum or signature appears. This is `SRV-003` working as designed rather than a special case.

| ID | Requirement | Pri | Rationale | Acceptance criteria | Verification | Depends on | Target |
|---|---|---|---|---|---|---|---|
| **SRV-001** | Host can provision a supported game server on their own PC, with no manual file editing and no administrator rights. [E-15] [E-16] | P0 | Removes the main setup barrier (G-7). | On a clean standard-user Windows VM, provisioning completes end to end (artifact downloaded and verified, configuration written, process started and reachable on the configured local port) without an admin prompt. | Manual VM test; integration test | SRV-002, SRV-003, INST-001, SEC-012 | Beta |
| **SRV-002** | Server artifacts are obtained only from official sources and verified before execution. [E-15] [E-16] | P0 | Supply-chain risk on the host's machine. | The source URL and verification method per game are recorded in `research.md`; a corrupted or tampered artifact is rejected and reported; no artifact is executed before verification succeeds. | Unit test; security test | — | Beta |
| **SRV-003** | Only games whose acquisition path has been verified can be provisioned. | P0 | No unverified claims (mirrors GAME-006 discipline). | The provisionable list equals the set of profiles with a passed acquisition experiment; a profile without one is absent from the provision flow and still supports manual local configuration; the UI states which games can be provisioned. | Unit test; release review | SRV-002, GAME-007 | Beta |
| **SRV-004** | A freshly provisioned server has query and RCON disabled; where the game supports them, enabling one requires an explicit host action and a strong generated secret. [E-15] [E-16] | P0 | No silent exposure. | **Java edition:** default provisioning writes `enable-rcon` and `enable-query` false; the server refuses RCON until the host enables it; a generated `rcon.password` is at least 128 bits of entropy and is not derived from user input. **Bedrock:** no RCON or query setting exists, so the app offers no such toggle and no credential is generated — see `research.md` RQ-13.3. | Unit test; security test | GAME-008, SEC-007 | Beta |
| **SRV-005** | The provisioned server's listen port and address are host-local configuration and are never sent to the control plane or the relay. | P0 | Protects INV-1 and ADR-003. | Request capture during provisioning and tunnel start contains no managed-server address or port; the schema and API scans used for INV-1 pass with provisioning enabled; a schema-review test finds no such column. | Integration test with request capture; schema review | SEC-006, INV-1 | Beta |
| **SRV-006** | The UI shows managed server state as its own indicator, separate from tunnel, local server, and players seen. | P0 | Extends FR-010 without merging facts. | Provisioning, starting, running, crashed, updating, and failed each display distinctly; changing the managed-server indicator never changes the other three and vice versa. | UI test | FR-010, UX-002 | Beta |
| **SRV-007** | The app supervises the server process: start, graceful stop, and bounded restart after an unexpected exit. [E-17] | P0 | Hosts cannot debug a headless process themselves. | An externally killed server is restarted automatically within the configured backoff limit; repeated crashes stop automatic restart and surface a log excerpt; stopping the app leaves no server process running. | Integration test; manual VM test | SRV-001, NET-008 | Beta |
| **SRV-008** | World and configuration data survive a restart, an update, and a rollback. [E-18] | P0 | Data loss would be unacceptable. | Restarting, updating, and rolling back each preserve the world; a backup taken by the app restores to a running server; a failed update leaves the previous version launchable with its world intact. | Integration test; staging exercise | SRV-001, INST-003 | Beta |
| **SRV-009** | Server version updates are verified and reversible, and are not applied silently while a tunnel is active. | P1 | Patching a live server is disruptive. | An update verifies the artifact before use, keeps the previous version, and can be rolled back; an update attempted during an active tunnel requires confirmation and states that guests will be disconnected. | Integration test; UI test | SRV-002, SRV-008 | Beta |
| **SRV-010** | Provisioning writes only inside a per-user data directory and never installs a service or requires elevation. | P0 | Least privilege. | Artifacts, world, and configuration live under the user's profile directory; no write outside it; no service, scheduled task, or startup entry is created; uninstall leaves no server process running. | Manual VM test | SEC-012, INST-001, INST-004 | Beta |
| **SRV-011** | A managed server that can report player joins directly may be shown as *confirmed* presence, separately from *observed traffic*. [E-15] | P2 | Honesty improves when the game itself answers. | For a managed Java server using the Server Management Protocol, join and leave notifications are labelled **confirmed by the game server** and are visually distinct from the traffic-based players-seen indicator; when the protocol is unavailable the UI falls back to observed traffic only and says so; **no game may be shown as "in game" on traffic evidence alone**. | UI test; integration test | FR-012, FR-010 | Beta |

**Addendum 2026-10-17 — `SRV-004` revisited after `research.md` RQ-13.4.** The recommended design changed: **do not enable RCON at all** (ADR-015), so no RCON credential is generated by the app. If the host wants management, the Server Management Protocol is enabled and **the game server generates its own secret into its own properties file**; the app neither creates nor holds it, and the TLS keystore password can be supplied by environment variable rather than written to disk. `SRV-004`'s original wording, which assumed the app generated an RCON password, is therefore superseded for Java. SEC-002 still needs the exception wording for a server-owned secret in the server's own file — that is ADR-014, still open.

**Open conflict to resolve at gate G1 (recorded, not resolved here).** SRV-004 requires a generated RCON secret that a running game server reads from its configuration **file**. SEC-002 states that secrets never appear in files. Either SEC-002 is amended with a documented exception for game-server credentials the server itself must read, or SRV-004 is dropped and RCON stays host-managed. **Narrowed 2026-10-17:** this now concerns **Java edition only** — Bedrock has no RCON (`research.md` RQ-13.3) — and the **Minecraft Server Management Protocol (MSMP)** may be a better mechanism than RCON, so MSMP should be evaluated before the choice is made. See `architecture.md` §13 and `research.md` ADR-014.

## Installation and update requirements

| ID | Requirement | Pri | Rationale | Acceptance criteria | Verification | Depends on | Target |
|---|---|---|---|---|---|---|---|
| **INST-001** | Windows install is per user and needs no administrator rights. | P0 | Least privilege. | Install and uninstall succeed on a standard-user account in a clean Windows VM. | Manual VM test | — | Beta |
| **INST-002** | The app updates itself from signed artifacts. | P0 | Security patches. | An update from version N to N+1 completes on a clean VM and the new version runs. | Staging exercise (E-9) | SEC-014 | Beta |
| **INST-003** | A failed update leaves the previous version working. | P0 | Recoverability. | Interrupted or tampered updates in test leave version N launchable. | Staging exercise (E-9) | INST-002 | Beta |
| **INST-004** | Uninstall removes the app and leaves no running tunnel processes. | P1 | Clean removal. | After uninstall no app processes or listeners remain; user data removal is offered. | Manual VM test | — | 1.0 |
| **INST-005** | A missing WebView2 runtime is detected and handled. | P1 | Tauri prerequisite. | On a VM without the runtime the installer or first launch resolves it or shows a clear instruction. | Manual VM test | — | Beta |

## Testing and release requirements

| ID | Requirement | Pri | Rationale | Acceptance criteria | Verification | Depends on | Target |
|---|---|---|---|---|---|---|---|
| **TEST-001** | Protocol parsing, state machines, authorization, and validation have unit tests. | P0 | Core correctness. | Each rule in `contracts.md` §§3, 10–12, 15 has at least one passing and one failing-input test. | Unit tests in CI | — | Alpha |
| **TEST-002** | Integration tests cover client, API, database, relay, and a local game-server endpoint. | P0 | End-to-end behavior. | The suite runs in CI against disposable services and passes. | CI | — | Alpha |
| **TEST-003** | Network-impairment tests run loss, jitter, latency, and blocked-UDP scenarios. | P0 | Real networks are imperfect. | A defined harness applies each impairment and the documented expected behavior is asserted. | Network test | — | Beta |
| **TEST-004** | A security test suite covers unauthorized access, expired tokens, cross-session isolation, destination validation, port exposure, rate limits, secret leakage, and resource exhaustion. | P0 | Security as a gate. | Every listed category has at least one automated test that passes; failures block release. | CI + security review | — | Beta |
| **TEST-005** | Game compatibility tests L0–L2 exist per game and run before each release. | P0 | Honest support claims. | Per-game test plans exist; L0/L1 are automated where tooling exists; L2 results are recorded per release. | Compatibility test run | — | Beta |
| **TEST-006** | A reproducible benchmark harness exists. | P0 | No unmeasured claims. | Harness runs on documented hardware/network and outputs the data behind PERF requirements. | Benchmark | — | Beta |
| **TEST-007** | CI gates: formatting, lint, tests, dependency scan, and migration apply on an empty database. | P0 | Quality gate. | A change failing any gate cannot merge. | CI configuration review | — | Alpha |
| **TEST-008** | Release checklist includes documentation consistency and traceability checks. | P0 | Docs drift. | The checklist is run for each release and every requirement has a mapped task and verification. | Release review | — | 1.0 |

## Appendix A — Traceability matrix (derived from `tasks.md`)

| Requirement | Verification method | Implementing tasks |
|---|---|---|
| FR-001 | Integration test; manual | T-040, T-081 |
| FR-002 | UI test | T-082 |
| FR-003 | Unit test; integration test with request capture | T-071, T-082 |
| FR-004 | API test | T-042 |
| FR-005 | Integration test | T-043, T-070 |
| FR-006 | Integration test | T-043 |
| FR-007 | API test | T-044, T-084 |
| FR-008 | Integration test | T-015, T-044, T-084 |
| FR-009 | Integration test; measurement | T-044, T-065, T-084 |
| FR-010 | UI test | T-083 |
| FR-011 | Unit + integration test with real and stopped servers | T-072 |
| FR-012 | Integration test | T-073 |
| FR-013 | Security test | T-075 |
| FR-014 | Network test | T-063 |
| FR-015 | UI test | T-085 |
| NET-001 | Network test | T-061 |
| NET-002 | Network test | T-010, T-062 |
| NET-003 | Network test | T-061 |
| NET-004 | Network test | T-063 |
| NET-005 | Network test | T-063, T-092 |
| NET-006 | Network + security test | T-064, T-096 |
| NET-007 | Unit + fuzz + security test | T-031, T-062, T-094 |
| NET-008 | Integration test | T-067 |
| NET-009 | Network test | T-016, T-066 |
| NET-010 | Network test | T-015, T-092 |
| NET-011 | Release review | T-021, T-121 |
| GAME-001 | Game compatibility test; manual L2 | T-011, T-090, T-091 |
| GAME-002 | Game compatibility test; security test | T-011, T-090, T-091 |
| GAME-003 | Game compatibility test; manual L2 | T-012, T-091 |
| GAME-004 | Game compatibility test; manual L2 | T-013, T-091 |
| GAME-005 | Capture analysis; game compatibility test | T-013, T-091 |
| GAME-006 | Release review; UI test | T-002, T-091, T-121 |
| GAME-007 | Unit test | T-032 |
| GAME-008 | Unit + integration test | T-032 |
| GAME-009 | Integration test | T-082 |
| GAME-010 | Experiment result; manual L2 | T-014 |
| SEC-001 | Integration + security test | T-017, T-040 |
| SEC-002 | Security test | T-017, T-074, T-093 |
| SEC-003 | Integration test | T-040 |
| SEC-004 | Unit + security test | T-050, T-060 |
| SEC-005 | Security test; code review | T-060, T-093 |
| SEC-006 | Unit + security test | T-071, T-093 |
| SEC-007 | Unit + security test | T-044 |
| SEC-008 | Security test | T-045, T-093 |
| SEC-009 | Security test | T-034, T-041, T-093 |
| SEC-010 | Security test | T-060, T-093 |
| SEC-011 | Doc/UI review | T-110 |
| SEC-012 | Manual VM test | T-100 |
| SEC-013 | Integration + security test | T-046, T-093 |
| SEC-014 | Security test (E-9) | T-019, T-101 |
| SEC-015 | Integration test; tabletop exercise | T-045, T-049, T-108 |
| SEC-016 | Staging exercise | T-050 |
| SEC-017 | CI check | T-001, T-030 |
| SEC-018 | Design review; security test | T-049 |
| REL-001 | Failure-injection test | T-097 |
| REL-002 | Failure-injection test | T-043, T-097 |
| REL-003 | Unit test | T-047 |
| REL-004 | Failure-injection test | T-048, T-097 |
| PERF-001 | Benchmark | T-095 |
| PERF-002 | Benchmark | T-095 |
| PERF-003 | Benchmark | T-018, T-095 |
| PERF-004 | Benchmark | T-095 |
| PERF-005 | Load test | T-096 |
| PERF-006 | Measurement | T-020 |
| PERF-007 | Measurement | T-065 |
| UX-001 | Usability test | T-081, T-087 |
| UX-002 | UI test; design review | T-083 |
| UX-003 | Review; UI test | T-085 |
| UX-004 | Manual + automated UI test | T-018, T-086 |
| UX-005 | Automated check | T-080 |
| UX-006 | UI test | T-083 |
| UX-007 | Automated check | T-080 |
| OPS-001 | Infrastructure review | T-104 |
| OPS-002 | Staging exercise | T-103 |
| OPS-003 | Staging exercise | T-105 |
| OPS-004 | Load test | T-064, T-096 |
| OPS-005 | Document review | T-107 |
| OPS-006 | Staging exercise | T-103 |
| OPS-007 | Restore exercise | T-106 |
| OPS-008 | Tabletop exercise | T-108 |
| PRIV-001 | Network capture test | T-098 |
| PRIV-002 | Security test | T-046, T-075 |
| PRIV-003 | Integration test | T-109, T-098 |
| PRIV-004 | Integration test | T-098 |
| PRIV-005 | Document review | T-110 |
| PRIV-006 | Schema review | T-003, T-033 |
| INST-001 | Manual VM test | T-100 |
| INST-002 | Staging exercise (E-9) | T-019, T-101 |
| INST-003 | Staging exercise (E-9) | T-019, T-101 |
| INST-004 | Manual VM test | T-102 |
| INST-005 | Manual VM test | T-100 |
| TEST-001 | Unit tests in CI | T-031, T-034 |
| TEST-002 | CI | T-070 |
| TEST-003 | Network test | T-010, T-092 |
| TEST-004 | CI + security review | T-093 |
| TEST-005 | Compatibility test run | T-090, T-121 |
| TEST-006 | Benchmark | T-095 |
| TEST-007 | CI configuration review | T-001, T-003, T-033 |
| TEST-008 | Release review | T-004, T-022, T-120, T-121, T-122 |
| SRV-001 | Manual VM test; integration test | T-023, T-024, T-025, T-088 |
| SRV-002 | Unit test; security test | T-023, T-024, T-035 |
| SRV-003 | Unit test; release review | T-023, T-024, T-035 |
| SRV-004 | Unit test; security test | T-035 |
| SRV-005 | Integration test with request capture; schema review | T-071, T-076 |
| SRV-006 | UI test | T-076, T-088 |
| SRV-007 | Integration test; manual VM test | T-025, T-076 |
| SRV-008 | Integration test; staging exercise | T-026, T-035 |
| SRV-009 | Integration test; UI test | T-026, T-088 |
| SRV-010 | Manual VM test | T-027, T-035 |
| SRV-011 | UI test; integration test | T-035, T-088 |

Audit result at generation time: every requirement maps to at least one task and has a stated verification method; every task references at least one existing requirement. Recomputed by hand on 2026-10-17 for the SRV additions — **not** re-run with the audit script, so it must be confirmed by T-120.
