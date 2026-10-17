# plan.md — Execution Plan Overview

**Status:** DRAFT v0.1. Overview only; `tasks.md` is the authoritative granular checklist. No dates or effort estimates exist yet, so this plan sequences work by dependency, not calendar.
**Last updated:** 2026-10-16

## Milestones and gates

| Milestone | Tasks | Exit gate (summary) |
|---|---|---|
| M0 — Foundations and documentation validation | T-001–T-004 (4 tasks) | Gate G0: tooling decided, sources upgraded, documentation reviewed. Exit when T-001 to T-004 are verified. |
| M1 — Feasibility experiments (prototype code only; not production) | T-010–T-027 (18 tasks) | Gate G1: ADR-001, ADR-002, ADR-007, ADR-004 decided from evidence; guest-admission design confirmed or replaced; ADR-012/013/014 decided so the SRV provisioning scope is evidence-based. |
| M2 — Skeleton, domain model, persistence | T-030–T-035 (6 tasks) | Gate G2: schema migrations apply; protocol crate and profile loader pass tests. |
| M3 — Authentication, authorization, and control-plane API | T-040–T-050 (11 tasks) | Gate G3: all routes in `api.md` implemented with schema-validated tests (admin routes covered by T-049, the last task of the milestone). |
| M4 — Tunnel protocol, TCP, UDP, and relay | T-060–T-067 (8 tasks) | Gate G4: TCP and UDP traffic flows end to end in the integration environment under tests. |
| M5 — Engine and client integration | T-070–T-076 (7 tasks) | Gate G5: engine drives full tunnel lifecycle against the staging stack. |
| M6 — Desktop UI and onboarding | T-080–T-088 (9 tasks) | Gate G6: first-tunnel flow works end to end through the UI. |
| M7 — Automated testing, compatibility, security, performance | T-090–T-097 (8 tasks) | Gate G7: compatibility, security, network, performance, and failure-injection tests pass; compatibility results recorded per game. (Privacy tests run in M8 after retention jobs exist: T-098.) |
| M8 — Packaging, observability, deployment, operations | T-098–T-110 (12 tasks) | Gate G8: staging environment passes exercises for deploy, rollback, backup, alerts, and updates. |
| M9 — Release readiness | T-120–T-122 (3 tasks) | Gate G9: go/no-go review. |

Task counts are computed from `tasks.md` data. Total: 86 tasks, 116 requirements.

## Sequencing

1. **M0** establishes tooling and upgrades weak research. Nothing is built.
2. **M1** runs the feasibility experiments. This is the main risk-reduction step: a failure here changes the architecture, so it comes before any production code.
3. **Gate G1 (T-022)** converts experiment results into decisions and updates the documents. Production work starts only after G1.
4. **M2–M3** (skeleton, persistence, control plane) and **M4** (relay, forwarding) can be developed in parallel by different people once G1 passes; M4 needs the signing keys from T-050.
5. **M5–M6** (engine, UI) follow M3/M4.
6. **M7** (testing) is partly parallel with M5–M6; **M8** (packaging, operations, privacy tests) follows.
7. **M9** closes with the audit and release checklist.

Computed longest prerequisite chain, **verified by script 2026-10-17** (`scripts/audit_traceability.py`): T-001 → T-002 → T-004 → T-010 → T-012 → T-020 → T-022 → T-030 → T-033 → T-034 → T-041 → T-042 → T-047 → T-070 → T-071 → T-072 → T-082 → T-083 → T-084 → T-091 → T-120 → T-121 → T-122 (**23 tasks**). It reflects dependency depth only; relay tasks T-060 to T-063 are not on this chain but are essential and gate M5.

## Decision gates and conditions

| Gate | Decision | Must be true to proceed |
|---|---|---|
| G0 | Planning documents are consistent and tooling chosen | Mermaid diagrams render; migration tool chosen; DDL runs on real PostgreSQL; reported game facts upgraded or still flagged |
| G1 | Commit to architecture | ADR-001 (UDP transport), ADR-002 (relay-first), ADR-007 and ADR-004 (guest model and relay visibility), ADR-005 (desktop stack), ADR-006 (updates), ADR-009 (sign-in), ADR-012/013/014 (managed server scope, first cut, credential handling) each Accepted or replaced, citing experiment results |
| G2 | Foundations usable | Migrations apply in CI; protocol and profile tests pass |
| G3 | Control plane complete | Every route in `api.md` (except admin) has schema-checked tests; isolation tests pass |
| G4 | Data plane works | TCP and UDP integration tests pass through the relay |
| G5 | Client lifecycle works | Engine drives start, reconnect, and stop against staging |
| G6 | UI usable | First-tunnel flow completes through the UI |
| G7 | Quality evidence | Compatibility (L0–L2 recorded), security, network, performance, and failure-injection suites pass |
| G8 | Operable | Deploy, rollback, backup, alert, and update exercises succeed in staging |
| G9 | Release | Audit shows zero unmapped requirements; all P0 verified; support claims match recorded L2 results |

## Major risks

| Risk | Impact | Mitigation task(s) | Early signal |
|---|---|---|---|
| UDP over QUIC datagrams performs poorly or hits MTU limits | UDP-first games unusable | T-010, T-016 | Experiment E-6 results |
| Source-IP guest admission fails on shared or changing addresses | Guest access design must change | T-015 | Experiment E-13 |
| A game's join flow does not work through a relay | That game unsupported | T-011 to T-014, T-091 | Experiments E-1 to E-5 |
| Relay egress cost is higher than expected | Product not economical | T-020, T-095, T-103 | Bytes per player-minute (E-10) |
| Public endpoints are abused | Cost, reputation, legal risk | T-045, T-049, T-064, T-107, T-108 | Abuse metrics in staging |
| Relay can read game traffic in the default model | Privacy and trust | T-110, ADR-004 | Review at G1 |
| Admin authentication undesigned | High-impact compromise | T-049 | Review before Beta |
| Dependence on weak or secondary sources | Wrong ports or behavior | T-002 | Items still tagged `[R]` |
| Tauri/WebView2 accessibility or update issues | UX and install problems | T-018, T-019 | Spike results |
| Game servers cannot be obtained or run legally, without elevation, or on the host's terms | The provisioning feature loses its reason to exist | T-023, T-024, T-027 | **Legally resolved for Minecraft Java, 2026-10-17** (`research.md` §5 RQ-13.1b): hosting and even charging for access are explicitly permitted. Remaining: E-15 technical acquisition, E-16 Bedrock, and the E-19 VM footprint |
| A managed server exposes something by default (RCON, query, world list, IPv6 listener) | Host's machine or LAN compromised | T-035, T-093 | Generated-configuration review |
| Managed server crash loop or disk growth degrades the host's PC | Users uninstall the app | T-025, T-026 | Restart limits and per-server footprint |
| Generated game-server credential contradicts SEC-002 | Requirement conflict blocks G1 | T-022 (ADR-014) | Terms of the credential decision |

## Deferred (not in the initial release)

Direct peer-to-peer upgrade (NET-011), guest-side client and end-to-end encryption, Windows service mode, non-Windows clients, L4D lobby support unless T-014 passes (GAME-010). Any of these entering scope requires a decision at a gate and a change to `requirements.md`.

## Links

`tasks.md`, `requirements.md`, `research.md`, `architecture.md`.
