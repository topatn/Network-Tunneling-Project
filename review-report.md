# review-report.md — Final Architecture Review Report

**Status:** Planning-phase review, 2026-10-16. This report states what was checked, how, and what remains unresolved. It does not claim that anything has been implemented or tested.

> **Superseded in part, 2026-10-17.** Host-local game server provisioning was added to scope (`context.md` G-7; amended NG-1). The counts in this report — 18 files, 99 requirements, 78 tasks — are **stale**: the set is now 110 requirements and 86 tasks across 10 milestones, and `requirements.md`, `tasks.md`, `plan.md`, `architecture.md`, `contracts.md`, `research.md`, `user-flows.md`, `DESIGN.md`, `ux.md`, `spec.md`, and this file's own §7 blockers all changed.
>
> **Corrections to §1 and §2, recorded 2026-10-17 rather than edited into the dated text:**
>
> 1. **The traceability audit is no longer hand-maintained.** `scripts/audit_traceability.py` exists, passes, and is itself proven against **13** planted defects. Result: 110 requirements, 110 traceability rows, 86 tasks, 19 experiments, 19 ADR summary rows (6 with full records), 7 invariants, 12 model/DDL tables, 20 routes cross-checked, longest chain 23 tasks, **0 errors, 23 warnings** (seven forward task references, which `tasks.md` rule 2 permits; thirteen draft ADRs awaiting their full records; three cited paths not yet written). This partly satisfies **T-120**; **API/data-model agreement is now covered** (12 tables compared column-by-column against `schema-draft.sql`, and 20 routes cross-checked against `contracts.md` for method, path, and success status — both defect directions proven), while **diagram agreement** and any check needing semantic judgement remain uncovered there.
> 2. **The Mermaid check is stronger than §1 claims.** All 11 diagrams now **render** with `@mermaid-js/mermaid-cli` 12.0.0, not merely parse with 10.9.8, and a malformed diagram was confirmed rejected. Visual layout is still unreviewed.
> 3. **A §2 claim cannot be verified and is wrong.** §2 lists "Extra: `schema-draft.sql`, `contracts/` (11 schemas + examples)" as created. `schema-draft.sql` is present; **`contracts/` and `contracts/examples.json` do not exist in the repository**, yet `contracts.md`'s status paragraph, `api.md`, `data-model.md` §7, and `tasks.md` T-031 all cite them as authoritative. Confirmed by directory listing on 2026-10-17. **The 11 JSON Schemas could not have been validated, because they are not here.** Treat `contracts.md` as the sole wire-format source of truth; T-031's "Done when" refers to a file that still has to be written.
> 4. **§2 understates the file set.** The repository also contains `Cargo.toml`, `Cargo.lock`, `clippy.toml`, `rustfmt.toml`, `deny.toml`, `.gitignore`, `.gitattributes`, `CONTRIBUTING.md`, `docs/dependency-register.md`, `scripts/audit_traceability.py`, `.github/workflows/ci.yml`, and six crate directories. The traceability audit in §1 was **not** re-run for the additions and is now hand-maintained; T-120 must confirm it. The §7 blocker list below also still needs a new entry for the acquisition path (E-15, E-16, E-19). This report is otherwise left as the dated record it is.

## 1. Scope and method

- **Input:** the architecture-planning prompt only. No existing repository, code, or prior documents were provided, so this was treated as a **new project**. This is an assumption: if a repository exists, a conflict review against it has not been done.
- **Research:** web searches and page reads on game ports, NAT traversal, QUIC and UDP tunneling, desktop frameworks, packaging, authentication, observability, test tooling, and hosting cost. Source quality is uneven; `research.md` labels each finding verified, reported, assumption, or experiment-needed.
- **Mechanical checks performed:**
  - SQL draft parsed with the PostgreSQL parser (syntax only; **not run** against a database).
  - 11 JSON Schemas validated; example payloads (valid and deliberately invalid) checked against them. String-format enforcement (`uuid`, `date-time`) was not independently confirmed.
  - 11 Mermaid diagrams parsed with Mermaid 10.9.8 (syntax only; visual rendering **not** checked). The parser was confirmed to reject invalid input.
  - Contrast ratios for both palettes computed with the WCAG formula, which was checked against reference values. Untested: hover/disabled states and high-contrast mode.
  - **Superseded 2026-10-17:** now recomputed from the token table by `scripts/check_contrast.py`, which self-validates against the three WCAG reference pairs and then re-derives all 22 pairs. All 22 matched, and the checker was confirmed to fail on a planted overstated ratio and a planted wrong threshold. Added as a CI job. Hover, disabled, and high-contrast states remain unchecked.
  - Reference audit: every requirement, task, experiment, ADR, invariant, section, and file reference in the documents resolves. The audit script was confirmed to detect planted bad references.
  - Traceability: all 99 requirements map to at least one of 78 tasks; every task cites existing requirements and prerequisites that precede it.
  - Semantic cross-checks: error codes (contracts vs schema), tunnel states (SQL vs schema), relay statuses, invitation bounds, engine states vs the design status table. One drift was found and fixed (UDP datagram header description in `architecture.md`).
- **Not checked:** whether the prose of the documents is semantically consistent beyond the checks above; compilation of any code; behavior of any game; real-world feasibility of any experiment.

## 2. Documentation coverage checklist

| Required file | Created | Depth / known gaps |
|---|---|---|
| `AGENTS.md` | Yes | Complete draft. Tooling commands are conventions until T-001 |
| `CLAUDE.md` | Yes | Complete draft; defers to `AGENTS.md` |
| `context.md` | Yes | Complete draft. Success-metric targets deliberately unset |
| `idea.md` | Yes | Complete draft |
| `README.md` | Yes | Complete for the planning phase; all run/test commands labeled not yet valid |
| `architecture.md` | Yes | Complete draft. Open design issues listed in §20 (fallback carrier, stable guest address, admission, region) |
| `data-model.md` | Yes | Complete draft. Invariants INV-2/4/5 need tests |
| `schema.md` | Yes | Complete; migration tool undecided |
| `contracts.md` | Yes | Complete draft. Admin API and identity-provider exchange are open |
| `api.md` | Yes | Complete; matches `contracts.md` routes (checked) |
| `DESIGN.md` | Yes | Provisional choices (owner has not chosen brand, themes, window behavior). Framework mapping open |
| `user-flows.md` | Yes | All 15 journeys. Copy is draft; per-game guest instructions unverified |
| `ux.md` | Yes | Complete |
| `requirements.md` | Yes | 99 requirements; numeric targets deliberately deferred to baselines |
| `spec.md` | Yes | Complete index |
| `tasks.md` | Yes | 78 tasks in 10 milestones |
| `plan.md` | Yes | Complete; no dates or effort estimates |
| `research.md` | Yes | **Partial**: see §5 gaps. ADRs are summary rows, not full records (T-022) |
| Extra: `schema-draft.sql`, `contracts/` (11 schemas + examples) | Yes | Supporting artifacts |

## 3. Completion criteria (prompt §10)

| Criterion | Status | Notes |
|---|---|---|
| Every required document created | Met | 18 files created; depth varies as above |
| Scope and initial games documented | Met | |
| Technology stack evaluated and justified | **Partially met** | Desktop and transport reasoned from weak sources; backend and database chosen by reasoning only (ADR-008, low confidence) |
| Architecture covers control plane, data plane, client, relay | Met | |
| TCP, UDP, NAT traversal, relay fallback, game networking addressed | **Partially met** | Relay-first only; direct P2P deferred; **fallback carrier for UDP-blocked networks not designed**; game facts mostly reported, not confirmed |
| Authentication, authorization, isolation, privacy, abuse documented | **Partially met** | Admin authentication undesigned; identity provider undecided; DDoS/abuse plan not researched (OPS-005) |
| Data models and contracts concrete | Met | Open items listed in `contracts.md` §16 |
| Design system and flows cover principal states | Met | Provisional design choices |
| Requirements measurable | Met | Targets for performance intentionally set after baselines |
| Tasks sequential, testable, linked | Met | Mechanically checked |
| Research findings and ADRs have supporting evidence | **Partially met** | Many findings are secondary sources; backend/DB, DDoS, and relay hosting alternatives thin; ADRs not full records |
| Cross-document consistency and traceability reviewed | Met (as scoped in §1) | Mechanical and listed semantic checks only |
| Unverified assumptions and risks listed | Met | `context.md` §10, `plan.md`, `research.md` |
| Coverage checklist shows every file's status | Met | §2 |
| Final review identifies blockers | Met | §7 |

## 4. Game compatibility

| Game | Required networking (reported) | Validation status | Known limitations |
|---|---|---|---|
| Terraria | TCP 7777 default; UDP need unresolved | **Untested**, source is a community wiki | UDP question (E-1) |
| Minecraft Java | TCP 25565; optional UDP query; optional TCP RCON 25575 | **Untested**, secondary sources | RCON/query must stay off by default |
| Minecraft Bedrock | UDP 19132 (v4) / 19133 (v6); LAN-visibility binds default ports | **Untested**, secondary sources | Non-default and IPv6 ports (E-2); console/mobile limits unverified (AS-2) |
| Left 4 Dead 2 | Source engine 27015 UDP, TCP for RCON | Engine ports from Valve's wiki; L4D2 specifics **untested** | Steam lobby joins through a relay unknown (E-4) |
| Left 4 Dead | TCP and UDP 27015 per one community image | **Untested**, weakest evidence | Needs capture (E-5) |

None is confirmed compatible. A game is called supported only after its real-client (L2) test passes.

## 5. Principal research gaps

1. Game facts rest largely on secondary sources; Mojang and Valve primary documentation still to be read (T-002).
2. RFC texts for QUIC datagrams and HTTP datagrams were confirmed from summaries, not read in full.
3. Backend framework, database, and migration tool: reasoning only.
4. DDoS and abuse mitigation options for relay hosting: not researched.
5. Hosting providers and regions: prices and allowances conflict between sources; target player regions undecided.
6. Identity provider options and passkey support: not researched.
7. ADR records incomplete (context/options/consequences per ADR).

## 6. Security review summary

| Principal risk | Planned mitigation | Status |
|---|---|---|
| Relay used as an open proxy | Relay never dials client-chosen targets (SEC-005) | Designed; untested |
| Engine reaching other local/LAN services | Engine dials only host-configured targets; local-target policy (SEC-006) | Designed; policy needs security review |
| Public relay port abused or scanned | Source-IP admission, limits, quotas | Admission viability unproven (E-13) |
| Stolen invitation / token replay | Hashed secrets, expiry, revocation, `jti` replay cache | Designed |
| Relay can read game traffic | Disclosed; end-to-end option deferred (ADR-004) | Open decision |
| Admin compromise | Separate stronger authentication | **Undesigned** (SEC-018) |
| Update supply chain | Signed updates, insecure options off | Untested (E-9) |
| Secret leakage in logs | Redaction, leak tests | Designed |

## 7. Blockers before implementation

1. Decide the guest model (ADR-007) and relay visibility (ADR-004). A default (stock-client guests, relay can read traffic) was assumed to keep the documents moving.
2. Run experiments E-6 (UDP over QUIC datagrams), E-13 (guest admission), and the per-game join experiments E-1 to E-5. Their results may change the architecture (gate G1, T-022).
3. Replace secondary game sources with primary documentation (T-002).
4. Decide identity provider, admin authentication model, backend stack (with evidence), and migration tool.
5. Design the fallback carrier for UDP-blocked networks (E-14), or explicitly accept the limitation.
6. Choose target regions and hosting provider; measure bytes per player-minute (E-10).
7. Owner confirmation of provisional design choices and of the P0 scope (81 of 99 requirements are P0; **now 90 of 109** after the SRV additions).
8. Confirm whether an existing repository or prior decisions must be reconciled.
9. **Partly resolved 2026-10-17.** Confirm the host-local provisioning scope is wanted before the E-15 to E-19 experiments are invested in, and confirm AS-11. **AS-11 is now answered from the primary source**: `research.md` §5 RQ-13.1b reads the EULA and Usage Guidelines in full, finds hosting explicitly permitted including charging for access, and records six binding constraints as requirements `SRV-012`–`SRV-017`. **Still needed:** owner or legal confirmation that our implementation complies — the quoted text is not legal advice — and the E-19 standard-user VM footprint check.

## 8. Requirement-by-requirement documentation audit

"Documented" means the requirement has an acceptance criterion, a verification method, and at least one mapped task (checked by script: **pass** for all 99). "Verified" means the product behavior was demonstrated: **not yet for any requirement**, because nothing is implemented.

| Requirement | Pri | Documented (criterion, verification, task) | Mapped tasks | Verified |
|---|---|---|---|---|
| FR-001 | P0 | pass | T-040, T-081 | not yet |
| FR-002 | P0 | pass | T-082 | not yet |
| FR-003 | P0 | pass | T-071, T-082 | not yet |
| FR-004 | P0 | pass | T-042 | not yet |
| FR-005 | P0 | pass | T-043, T-070 | not yet |
| FR-006 | P0 | pass | T-043 | not yet |
| FR-007 | P0 | pass | T-044, T-084 | not yet |
| FR-008 | P0 | pass | T-015, T-044, T-084 | not yet |
| FR-009 | P0 | pass | T-044, T-065, T-084 | not yet |
| FR-010 | P0 | pass | T-083 | not yet |
| FR-011 | P0 | pass | T-072 | not yet |
| FR-012 | P1 | pass | T-073 | not yet |
| FR-013 | P1 | pass | T-075 | not yet |
| FR-014 | P0 | pass | T-063 | not yet |
| FR-015 | P1 | pass | T-085 | not yet |
| NET-001 | P0 | pass | T-061 | not yet |
| NET-002 | P0 | pass | T-010, T-062 | not yet |
| NET-003 | P0 | pass | T-061 | not yet |
| NET-004 | P0 | pass | T-063 | not yet |
| NET-005 | P0 | pass | T-063, T-092 | not yet |
| NET-006 | P0 | pass | T-064, T-096 | not yet |
| NET-007 | P0 | pass | T-031, T-062, T-094 | not yet |
| NET-008 | P0 | pass | T-067 | not yet |
| NET-009 | P1 | pass | T-016, T-066 | not yet |
| NET-010 | P1 | pass | T-015, T-092 | not yet |
| NET-011 | P3 | pass | T-021, T-121 | not yet |
| GAME-001 | P0 | pass | T-011, T-090, T-091 | not yet |
| GAME-002 | P0 | pass | T-011, T-090, T-091 | not yet |
| GAME-003 | P0 | pass | T-012, T-091 | not yet |
| GAME-004 | P0 | pass | T-013, T-091 | not yet |
| GAME-005 | P0 | pass | T-013, T-091 | not yet |
| GAME-006 | P0 | pass | T-002, T-091, T-121 | not yet |
| GAME-007 | P0 | pass | T-032 | not yet |
| GAME-008 | P0 | pass | T-032 | not yet |
| GAME-009 | P0 | pass | T-082 | not yet |
| GAME-010 | P2 | pass | T-014 | not yet |
| SEC-001 | P0 | pass | T-017, T-040 | not yet |
| SEC-002 | P0 | pass | T-017, T-074, T-093 | not yet |
| SEC-003 | P0 | pass | T-040 | not yet |
| SEC-004 | P0 | pass | T-050, T-060 | not yet |
| SEC-005 | P0 | pass | T-060, T-093 | not yet |
| SEC-006 | P0 | pass | T-071, T-093 | not yet |
| SEC-007 | P0 | pass | T-044 | not yet |
| SEC-008 | P0 | pass | T-045, T-093 | not yet |
| SEC-009 | P0 | pass | T-034, T-041, T-093 | not yet |
| SEC-010 | P0 | pass | T-060, T-093 | not yet |
| SEC-011 | P0 | pass | T-110 | not yet |
| SEC-012 | P0 | pass | T-100 | not yet |
| SEC-013 | P0 | pass | T-046, T-093 | not yet |
| SEC-014 | P0 | pass | T-019, T-101 | not yet |
| SEC-015 | P0 | pass | T-045, T-049, T-108 | not yet |
| SEC-016 | P0 | pass | T-050 | not yet |
| SEC-017 | P1 | pass | T-001, T-030 | not yet |
| SEC-018 | P0 | pass | T-049 | not yet |
| REL-001 | P0 | pass | T-097 | not yet |
| REL-002 | P0 | pass | T-043, T-097 | not yet |
| REL-003 | P0 | pass | T-047 | not yet |
| REL-004 | P1 | pass | T-048, T-097 | not yet |
| PERF-001 | P0 | pass | T-095 | not yet |
| PERF-002 | P0 | pass | T-095 | not yet |
| PERF-003 | P1 | pass | T-018, T-095 | not yet |
| PERF-004 | P1 | pass | T-095 | not yet |
| PERF-005 | P0 | pass | T-096 | not yet |
| PERF-006 | P0 | pass | T-020 | not yet |
| PERF-007 | P0 | pass | T-065 | not yet |
| UX-001 | P0 | pass | T-081, T-087 | not yet |
| UX-002 | P0 | pass | T-083 | not yet |
| UX-003 | P1 | pass | T-085 | not yet |
| UX-004 | P0 | pass | T-018, T-086 | not yet |
| UX-005 | P0 | pass | T-080 | not yet |
| UX-006 | P1 | pass | T-083 | not yet |
| UX-007 | P2 | pass | T-080 | not yet |
| OPS-001 | P0 | pass | T-104 | not yet |
| OPS-002 | P0 | pass | T-103 | not yet |
| OPS-003 | P0 | pass | T-105 | not yet |
| OPS-004 | P0 | pass | T-064, T-096 | not yet |
| OPS-005 | P0 | pass | T-107 | not yet |
| OPS-006 | P1 | pass | T-103 | not yet |
| OPS-007 | P0 | pass | T-106 | not yet |
| OPS-008 | P0 | pass | T-108 | not yet |
| PRIV-001 | P0 | pass | T-098 | not yet |
| PRIV-002 | P0 | pass | T-046, T-075 | not yet |
| PRIV-003 | P0 | pass | T-109, T-098 | not yet |
| PRIV-004 | P0 | pass | T-098 | not yet |
| PRIV-005 | P0 | pass | T-110 | not yet |
| PRIV-006 | P1 | pass | T-003, T-033 | not yet |
| INST-001 | P0 | pass | T-100 | not yet |
| INST-002 | P0 | pass | T-019, T-101 | not yet |
| INST-003 | P0 | pass | T-019, T-101 | not yet |
| INST-004 | P1 | pass | T-102 | not yet |
| INST-005 | P1 | pass | T-100 | not yet |
| TEST-001 | P0 | pass | T-031, T-034 | not yet |
| TEST-002 | P0 | pass | T-070 | not yet |
| TEST-003 | P0 | pass | T-010, T-092 | not yet |
| TEST-004 | P0 | pass | T-093 | not yet |
| TEST-005 | P0 | pass | T-090, T-121 | not yet |
| TEST-006 | P0 | pass | T-095 | not yet |
| TEST-007 | P0 | pass | T-001, T-003, T-033 | not yet |
| TEST-008 | P0 | pass | T-004, T-022, T-120, T-121, T-122 | not yet |

Summary: 99 of 99 requirements documented and mapped; 0 of 99 verified (implementation has not started).
