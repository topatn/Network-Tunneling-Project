# tasks.md — Implementation Checklist

**Status:** DRAFT v0.1.1. Authoritative implementation checklist. Requirements: `requirements.md`. Execution overview: `plan.md`.
**Last updated:** 2026-10-17 (SRV host-local provisioning tasks added; see `context.md` G-7)

## Rules for using this checklist

1. **A task is complete only when its "Done when" condition is demonstrated by its stated verification.** Code existing is not completion.
2. Tasks list their prerequisites explicitly and **the Prerequisites field is the authoritative ordering**, not the task number. Task IDs are stable identifiers: they are never reused or renumbered, and a task may legitimately depend on a higher-numbered task that was added later. Do not start a task before its prerequisites are verified. `scripts/audit_traceability.py` checks this — it reports a genuine prerequisite **cycle** as an error, and a forward reference as a warning.
3. Each task is bounded so an agent can finish it without making broad architectural decisions. If a task needs such a decision, stop and record it as an ADR in `research.md`.
4. Experiments (M1) produce **evidence and decisions, not production code**. Prototype code stays in a scratch area and is not promoted without a task of its own.
5. `[CP]` marks a critical-path task. Gate tasks update documents when evidence changes assumptions.
6. Report which checks were actually run (see `AGENTS.md`).

Counts: 86 tasks across 10 milestones.


## M0 — Foundations and documentation validation

*Gate G0: tooling decided, sources upgraded, documentation reviewed. Exit when T-001 to T-004 are verified.*

### T-001 — Create repository and CI skeleton [CP]

- [ ] **Prerequisites:** none
- **Scope:** Initialize repository, workspace layout from `architecture.md` §4, formatting and lint config, empty CI pipeline with a dependency-scan step; place `README.md`, `AGENTS.md`, and `CLAUDE.md` at the repository root and fix their relative links.
- **Deliverables:** Repo, CI config, CONTRIBUTING pointer to `AGENTS.md`.
- **Requirements:** TEST-007, SEC-017
- **Verification:** CI run on an empty commit
- **Progress 2026-10-17 — ALMOST complete. Two conditions still unmet.** Installed `rustc 1.99.1` via rustup (both `x86_64-pc-windows-msvc` and `x86_64-pc-windows-gnu`). `git init` and two commits made. Directory skeleton per `architecture.md` §4; root `Cargo.toml` workspace plus six crate manifests with deliberately **empty** `[dependencies]`; six `src/lib.rs` crate roots; `rustfmt.toml`, `clippy.toml`, `deny.toml`, `.gitignore`, `.gitattributes`; `.github/workflows/ci.yml`; `CONTRIBUTING.md`; `docs/dependency-register.md`; `scripts/audit_traceability.py`; README/AGENTS/CLAUDE confirmed at repository root. **Verified by running it** (`rustc 1.99.1`): `cargo metadata`, `cargo check`, `cargo fmt --all -- --check`, `cargo clippy -- -D warnings` (clean), `cargo test` under the GNU toolchain (0 tests), and **`cargo deny check advisories bans licenses` using the official prebuilt cargo-deny 0.20.2 Windows binary with its published SHA-256 verified**. **Three gates were each proven to fail when they should:** the lint gate denies `unwrap()` (exit 101), the dependency gate reports `RUSTSEC-2020-0071` on a seeded `time 0.1.45` (exit 1), and the traceability audit detects 5 of 5 planted defects. **Three configuration bugs were caught only by running the checks** — an invalid rustc lint in `clippy.toml`, two non-existent clippy lint names that would have silently enforced nothing, and a `deny.toml` that had never been parsed and used a key removed in cargo-deny 0.20. **Still NOT met, so the box stays unchecked:** (1) **CI has never run on a hosted runner** — no Git remote or CI provider is configured, so the "CI run on an empty commit" verification cannot be satisfied and no merge is gated; (2) the migration-apply job is disabled until T-033. **Environment gaps that remain** (not blocking T-001's gates, but blocking other work): no Windows SDK, so MSVC cannot link, and the GNU toolchain has no `dlltool.exe`, so neither toolchain can compile Rust tooling from source.

**Recorded inconsistency (decide at T-004):** `architecture.md` §4 describes `docs/` as holding the documentation set, but the 20 documents live at the repository root and only `docs/dependency-register.md` is under `docs/`. Not changed here — moving 20 files would break every relative link and is outside T-001's scope.
- **Done when:** CI runs format, lint, and dependency scan steps and passes on the skeleton; a seeded known-vulnerable test dependency makes the scan fail.

### T-002 — Replace reported game facts with primary sources [CP]

- [ ] **Prerequisites:** T-001
- **Scope:** Read Mojang/minecraft.wiki and Valve documentation; upgrade or correct every `[R]` item in `research.md` §2; update GAME requirement notes if facts change.
- **Deliverables:** Updated `research.md`; change list.
- **Requirements:** GAME-006
- **Verification:** Reviewer checks each §2 row cites a primary source or is still labeled `[R]`
- **Done when:** Every game row is either cited to a primary source or explicitly still `[R]` with a linked experiment.
- **Progress 2026-10-17 — PARTIAL. NOT complete.** Read this session: Minecraft Wiki `server.properties` (the shipped default files for Java 26.3 and BDS 1.26.52.3, both editions, in full), minecraft.wiki "Server/Requirements", "Server.jar", "Version_manifest.json", "Bedrock Dedicated Server", "Minecraft Server Management Protocol"; Microsoft Learn BDS getting-started and BDS properties; Terraria Wiki "Server"; Left4DevOps `l4d2-docker`. **Upgraded to `[V]`:** Minecraft Java `server-port` TCP 25565, `query.port` UDP 25565, `rcon.port` TCP 25575 and the `enable-rcon=false` / `enable-query=false` defaults; Minecraft Bedrock 19132/19133 and its property set (two independent sources); Terraria TCP 7777. **Added §2.1** (server defaults that change our security posture — Terraria's `upnp=1` auto-port-forward is the headline; also JMX, `server-udp-ports`, and Minecraft's `prevent-proxy-connections`) and **§2.3** (Terraria's UDP question, with a reading that the TCP/UDP mentions are defensive forwarding advice rather than a UDP listener — explicitly left `[X]`). **§2.2 is the important new finding:** Source-engine games identify players **by IP**, and because our engine dials the game server from one local address, all guests may collapse into a single identity and the game's own RCON brute-force ban could lock the host out. Raised as `AS-12`, an `architecture.md` open issue (13, high priority), and a threat-model row. **Corrected my own earlier error:** I had written that Bedrock's `server-udp-ports` did not exist; it does, in the shipped file, and the original `[R]` claim was sound. **Still `[R]`, so NOT done:** Terraria's TCP-vs-UDP contradiction (E-1); **all Valve/Source `[V]` rows could not be re-verified — `developer.valvesoftware.com` now returns an Anubis anti-bot interstitial**, so those labels rest on the 2026-10-16 reading; and a Bedrock `allow-list` default conflict between two sources. Nothing was executed against a running server.

### T-003 — Choose migration tool and verify DDL on real PostgreSQL [CP]

- [ ] **Prerequisites:** T-001
- **Scope:** Select a migration tool (record as ADR); run `schema-draft.sql` on real PostgreSQL versions; record supported versions and any fixes.
- **Deliverables:** ADR; corrected `schema-draft.sql`; version matrix.
- **Requirements:** TEST-007, PRIV-006
- **Verification:** Run DDL on each target version
- **Done when:** DDL applies cleanly to an empty database on every version in the matrix; no email column exists.
- **Progress 2026-10-17 — PARTIAL. NOT complete. Blocker removed; DDL still never run.** **ADR-008 was Accepted on 2026-10-17** (Rust control plane with `axum`, PostgreSQL via `sqlx`), so the gating decision this task was waiting on is now made. Consequences recorded: the workspace MSRV rises **1.77 → 1.94** (`sqlx` declares `rust-version = "1.94.0"`, confirmed on the released `v0.9.0` tag, not only on `main`), and the control plane gains a 145-crate dependency tree that T-030 must register. **The migration tool is still not chosen** — RQ-14 keeps `sqlx migrate`, `refinery`, and hand-rolled numbered SQL open, and choosing between them is this task's first deliverable. **What remains for "Done when":** run `schema-draft.sql` against a real PostgreSQL on every version in the version matrix and confirm no email column exists (PRIV-006). **No PostgreSQL instance is available in this environment**, and the schema has still never been executed — only parser-checked. See `research.md` §5 RQ-14.

### T-004 — Independent documentation review (gate G0) [CP]

- [ ] **Prerequisites:** T-002, T-003
- **Scope:** Review all planning documents for contradictions; validate Mermaid diagrams render; fix findings.
- **Deliverables:** Review report; fixed docs.
- **Requirements:** TEST-008
- **Verification:** Mermaid rendering run; checklist against `docs` consistency rules
- **Done when:** All diagrams render without error; review report lists zero unresolved contradictions.


## M1 — Feasibility experiments (prototype code only; not production)

*Gate G1: ADR-001, ADR-002, ADR-007, ADR-004 decided from evidence; guest-admission design confirmed or replaced; ADR-012, ADR-013, ADR-014 decided so the SRV provisioning scope is evidence-based.*

### T-010 — Experiment E-6: QUIC datagram relay under impairment [CP]

- [ ] **Prerequisites:** T-004
- **Scope:** Build a throwaway relay and engine; relay UDP through QUIC datagrams; apply loss, jitter, delay, and MTU limits; record round-trip behavior and drop counts.
- **Deliverables:** Experiment report in `research.md`; prototype code in a scratch directory.
- **Requirements:** NET-002, TEST-003
- **Verification:** Run experiment matrix; results table
- **Done when:** Report records results for each impairment case; ADR-001 status changed to Accepted or Rejected with data.

### T-011 — Experiments E-1: Terraria and Minecraft Java through a naive relay

- [ ] **Prerequisites:** T-004
- **Scope:** Forward TCP through a prototype relay for both games; test Terraria with and without UDP; test Java query/RCON mapping behavior.
- **Deliverables:** Report; Terraria UDP answer.
- **Requirements:** GAME-001, GAME-002
- **Verification:** Real client join for each game
- **Done when:** Both games joined by a real client through the prototype, and the Terraria UDP question is answered with evidence.

### T-012 — Experiment E-2: Bedrock through a UDP relay [CP]

- [ ] **Prerequisites:** T-010
- **Scope:** Test join on default and changed ports, IPv6 port behavior, LAN-visibility binding, and console/mobile behavior (AS-2).
- **Deliverables:** Report.
- **Requirements:** GAME-003
- **Verification:** Real client join; capture
- **Done when:** Report states pass/fail per case with captures; AS-2 resolved or marked unresolvable.

### T-013 — Experiments E-3 and E-5: Source engine (L4D2, L4D) direct connect and protocol capture

- [ ] **Prerequisites:** T-010
- **Scope:** Test `connect` through the relay for L4D2 and L4D; capture L4D traffic to establish required ports and protocols.
- **Deliverables:** Report with capture summary.
- **Requirements:** GAME-004, GAME-005
- **Verification:** Real client join; capture analysis
- **Done when:** Each game's required ports/protocols are documented from capture, and direct connect result recorded.

### T-014 — Experiment E-4: L4D2 lobby join through a relay

- [ ] **Prerequisites:** T-013
- **Scope:** Attempt Steam lobby and matchmaking joins with a relay-presented address; record failures.
- **Deliverables:** Report; GAME-010 decision.
- **Requirements:** GAME-010
- **Verification:** Real client attempts
- **Done when:** Outcome recorded; GAME-010 either confirmed in scope or documented as unsupported.

### T-015 — Experiment E-13: source-IP guest admission viability

- [ ] **Prerequisites:** T-010
- **Scope:** Test admission with IPv4/IPv6 mismatch, shared NAT, mobile network changes, and carrier-grade NAT; propose alternatives if it fails.
- **Deliverables:** Report; ADR-007 decision input.
- **Requirements:** FR-008, NET-010
- **Verification:** Tests on at least the network types named
- **Done when:** Report covers each named network type; decision recorded to keep or replace the admission mechanism.

### T-016 — Experiment E-14: fallback carrier when UDP to the relay is blocked

- [ ] **Prerequisites:** T-010
- **Scope:** Prototype a TCP- or port-443-based carrier; measure added latency and feasibility.
- **Deliverables:** Report; design sketch.
- **Requirements:** NET-009
- **Verification:** Blocked-UDP test network
- **Done when:** Report records whether a fallback is viable and its limits; decision recorded.

### T-017 — Experiment E-12: sign-in spike

- [ ] **Prerequisites:** T-004
- **Scope:** System browser with loopback redirect and PKCE; device-flow fallback; credential-store round trip.
- **Deliverables:** Report; ADR-009 update.
- **Requirements:** SEC-001, SEC-002
- **Verification:** Manual run on clean Windows VM
- **Done when:** Sign-in completes and a stored token is retrievable after restart; no secret found in files by scan.

### T-018 — Experiment E-8: desktop shell spike

- [ ] **Prerequisites:** T-004
- **Scope:** Thin Tauri shell with loopback engine; measure startup and idle memory; test screen reader and keyboard behavior.
- **Deliverables:** Report; ADR-005 update.
- **Requirements:** PERF-003, UX-004
- **Verification:** Measurements on defined hardware
- **Done when:** Numbers and accessibility findings recorded; ADR-005 confirmed or fallback chosen.

### T-019 — Experiment E-9: update, failure, rollback spike

- [ ] **Prerequisites:** T-004
- **Scope:** Signed update, interrupted update, tampered artifact, per-user install on clean VM.
- **Deliverables:** Report; ADR-006 update.
- **Requirements:** INST-002, INST-003, SEC-014
- **Verification:** Clean VM runs
- **Done when:** Tampered artifact is rejected and interrupted update leaves the old version launchable.

### T-020 — Experiment E-10: bytes per player-minute [CP]

- [ ] **Prerequisites:** T-011, T-012, T-013
- **Scope:** Measure traffic volume for each game during realistic play sessions.
- **Deliverables:** Measurement report; first cost model.
- **Requirements:** PERF-006
- **Verification:** Capture counters
- **Done when:** Report has numbers for all five games with method; cost model draft uses them.

### T-021 — Experiment E-7: direct peer-to-peer success rate (knowledge only)

- [ ] **Prerequisites:** T-010
- **Scope:** Measure direct-path success on target-type networks; keep the product relay-only.
- **Deliverables:** Report; ADR-002 follow-up.
- **Requirements:** NET-011
- **Verification:** Measurements
- **Done when:** Report recorded; no direct-connect code path enabled in any build.

### T-023 — Experiment E-15: Minecraft Java server artifact acquisition [CP]

- [ ] **Prerequisites:** T-004
- **Scope:** Establish the official distribution channel and the terms governing it; determine whether unattended download is permitted; find the published integrity mechanism; determine whether a standard user can install and run it with no elevation and no writes outside the profile directory; establish whether a JRE is required and how one may be provided without a system-wide install. Read the sources listed in `research.md` §9 "Sources to read for RQ-13" and record them with access dates.
- **Deliverables:** Experiment report in `research.md`; RQ-13 §1–§2 answered; artifact channel and verification method recorded.
- **Requirements:** SRV-001, SRV-002, SRV-003
- **Verification:** Run the download and integrity check on a clean standard-user VM
- **Progress 2026-10-17 — desk research only, NOT complete.** Read: minecraft.wiki "server.jar", "Server/Requirements", "Version_manifest.json". Established: EULA-licensed and free of charge; content-addressed SHA-1 download path; a `sha1` field in `version_manifest_v2.json`; `eula.txt` needs an explicit user `true`; `--initSettings` creates configuration without starting; RCON/query keys confirmed to exist; **JRE 21 required from 1.20.5, so a second vendor artifact is needed**; MSMP exists since 1.21.9. Still required: fetch the real manifest and verify a checksum end to end; find the exact current JRE floor and a no-elevation way to supply it; read the MSMP documentation; **read the EULA and Usage Guidelines, which the unreachable `minecraft.net` prevented.**
- **Done when:** Report states the channel, the terms, the integrity method, and whether the no-elevation constraint holds; ADR-013 confirmed or the game removed from the first cut.

### T-024 — Experiment E-16: Minecraft Bedrock server artifact acquisition [CP]

- [ ] **Prerequisites:** T-004
- **Scope:** As T-023 for the Bedrock Dedicated Server, plus the license-acceptance step and which settings a generated configuration must set (`server-port`, `server-portv6`, `level-name`, `enable-lan-visibility`, `online-mode`, `allow-list`). Record the IPv6 listener behavior, which GAME-003 and E-2 also depend on. **Corrected 2026-10-17:** do **not** look for `enable-rcon` or `enable-query` — BDS has neither (`research.md` RQ-13.3). **Reframed 2026-10-17:** Bedrock is out of the provisioning first cut (ADR-013), so this task's primary job is now to **find a published integrity mechanism for the BDS artifact**, and secondarily to settle GAME-003, E-2 and `FR-011`. It is no longer a prerequisite for shipping provisioning, but it remains a prerequisite of G1 because GAME-003 and the local probe depend on it.
- **Deliverables:** Experiment report; generated-configuration inventory.
- **Requirements:** SRV-001, SRV-002, SRV-003
- **Verification:** Run the download and integrity check on a clean standard-user VM
- **Progress 2026-10-17 — desk research only, NOT complete.** Read: Microsoft Learn "Getting Started with Bedrock Dedicated Server" and "Bedrock Dedicated Server Properties" (both vendor-primary), minecraft.wiki "Bedrock Dedicated Server". Established: ZIP distribution; **no RCON and no query exist in BDS**, confirmed by their absence from the official reference and independently by the wiki; `enable-lan-visibility` defaults true and binds 19132/19133 even when ports differ (upgrades a `[R]` claim to `[V]` as documented); `online-mode` defaults true and Microsoft recommends it for internet-facing servers; **a `server-udp-ports` setting cited previously does not appear in the official reference — treat as unverified**; **console and mobile players cannot connect by address, so AS-2 is answered no**; `/reload` unsupported, so configuration changes need stop/write/start. **Three blocking unknowns:** (1) no integrity mechanism found and the download page was unreachable, so **SRV-002 cannot currently be met for Bedrock and SRV-003 would exclude it**; (2) the Windows Defender firewall prompt on first start versus the no-elevation promise; (3) whether the documented loopback restriction affects an ordinary process, and therefore FR-011's probe.
- **Done when:** Report states channel, terms, integrity method, license-acceptance step, and the required settings; ADR-013 confirmed or the game removed from the first cut.

### T-025 — Experiment E-17: supervising a headless server process

- [ ] **Prerequisites:** T-023, T-024
- **Scope:** Start, health-check, graceful stop, bounded crash restart, log capture, and exit-code handling for each provisioned server. Measure restart behaviour under repeated kills.
- **Deliverables:** Report; supervision design sketch; ADR-012 update.
- **Requirements:** SRV-007
- **Verification:** Run each case against a real server process
- **Done when:** Report records restart behaviour, backoff limits, and log handling for both games; ADR-012 confirmed or revised.

### T-026 — Experiment E-18: update, rollback, world backup, footprint

- [ ] **Prerequisites:** T-025
- **Scope:** Update each server to a newer version and back; confirm the world survives update and rollback; measure disk, idle CPU, and memory per server. Record whether an update requires the tunnel to stop.
- **Deliverables:** Report; footprint numbers; cost-of-ownership note for the host's machine.
- **Requirements:** SRV-008, SRV-009
- **Verification:** Real update and rollback runs with world verification
- **Done when:** Update, rollback, and world preservation are demonstrated for both games with recorded evidence; per-server footprint is measured.

### T-027 — Experiment E-19: terms and standard-user footprint

- [ ] **Prerequisites:** T-004
- **Scope:** Record the license and end-user terms governing the host downloading and operating these servers on their own machine, with an access date and the exact text location. Confirm on a clean standard-user VM that nothing is written outside the profile directory and that no service or startup entry is created.
- **Deliverables:** Terms record with sources; VM checklist result; ADR-012 evidence.
- **Requirements:** SRV-010
- **Verification:** Clean standard-user VM inspection
- **Progress 2026-10-17 — BLOCKED, nothing verified.** `minecraft.net/en-us/eula`, `/en-us/usage-guidelines` and `aka.ms/MinecraftEULA` all timed out or returned transport errors from the research environment; the terms were **not read** and no substitute was used. AS-11 stays `[X]`. This task needs a person with working access to the official pages, or legal counsel. The one design decision that does **not** depend on the outcome: the app must never accept the EULA on the host's behalf — `eula.txt` (Java) and the BDS license step remain deliberate user actions.
- **Done when:** Terms are recorded with citations and an access date; the VM check shows no elevation and no writes outside the profile directory.

### T-022 — Decision gate G1: update ADRs from evidence [CP]

- [ ] **Prerequisites:** T-010, T-011, T-012, T-013, T-015, T-016, T-017, T-018, T-019, T-020, T-023, T-024, T-025, T-026, T-027
- **Scope:** Update ADR-001, 002, 004, 005, 006, 007, 009, 012, 013, 014; expand every ADR (including 003, 008, 010, 011) into a full record with context, options, decision, rationale, consequences, and date; revise affected documents (architecture, contracts, data model); record scope changes. **Resolve the SEC-002 / SRV-004 conflict** (ADR-014): either amend SEC-002 with a documented exception for game-server credentials, or drop generated RCON and leave it host-managed. Confirm or revise the SRV-010, SRV-002, SRV-007, SRV-008, and SRV-009 requirements against experiment evidence.
- **Deliverables:** Updated ADRs and documents; G1 decision memo.
- **Requirements:** TEST-008, SRV-001, SRV-002, SRV-003, SRV-007, SRV-008, SRV-009, SRV-010
- **Verification:** Review against experiment reports
- **Done when:** Each ADR status updated with cited evidence; documents agree after update (re-run T-004 checks); ADR-014 closed.


## M2 — Skeleton, domain model, persistence

*Gate G2: schema migrations apply; protocol crate and profile loader pass tests.*

### T-030 — Create project skeleton and dependency policy [CP]

- [ ] **Prerequisites:** T-022
- **Scope:** Create crates and app directories; record each dependency's purpose and maintenance notes; pin versions.
- **Deliverables:** Workspace, dependency register.
- **Requirements:** SEC-017
- **Verification:** CI scan and register review
- **Done when:** Every dependency in the lockfile appears in the register with a purpose.

### T-031 — Implement protocol crate: messages, framing, validation

- [ ] **Prerequisites:** T-030
- **Scope:** Message types and parsers for `contracts.md` §§10–11; JSON Schema conformance tests; malformed-input handling.
- **Deliverables:** `crates/protocol` with tests.
- **Requirements:** TEST-001, NET-007
- **Verification:** Unit tests including invalid inputs
- **Done when:** All example payloads in `contracts/examples.json` pass or fail as specified; malformed frames return errors without panic.
- **Correction 2026-10-17 — the referenced files do not exist.** `contracts/examples.json` and `docs/contracts/` are absent from this repository, although this task, `contracts.md`, `api.md`, and `data-model.md` §7 all cite them as authoritative. **This task must therefore *author* the JSON Schemas and the example payloads before it can validate against them**; the validation cannot be inherited from any earlier pass, because none was ever run. The withdrawn claim is recorded in `contracts.md`'s status paragraph and in `review-report.md`. Writing the schemas is a prerequisite to this task's own verification, not part of it.

### T-032 — Implement profile loader and five profile files

- [ ] **Prerequisites:** T-030
- **Scope:** Declarative, versioned profile format; validation at load; default-off optional mappings; custom port support.
- **Deliverables:** `crates/profiles`, `profiles/*.toml|json` for five games.
- **Requirements:** GAME-007, GAME-008
- **Verification:** Unit tests
- **Done when:** Invalid profile is rejected; each game's default-enabled mappings match its profile; a sixth sample profile loads with no code change.

### T-033 — Create migrations from the data model [CP]

- [ ] **Prerequisites:** T-003, T-030
- **Scope:** Translate `schema-draft.sql` to the chosen migration tool; add CI step applying all migrations to an empty DB; retire the draft file per `schema.md`.
- **Deliverables:** `migrations/`; CI step.
- **Requirements:** TEST-007, PRIV-006
- **Verification:** CI migration job
- **Done when:** Migrations apply to an empty database in CI; `schema-draft.sql` removed in the same change.

### T-034 — Implement persistence layer and invariant tests [CP]

- [ ] **Prerequisites:** T-033
- **Scope:** Data access for all entities; tests for INV-1 to INV-7 including INV-2, INV-4, INV-5 application checks.
- **Deliverables:** Persistence module and tests.
- **Requirements:** SEC-009, TEST-001
- **Verification:** Integration tests on a real database
- **Done when:** Each invariant has a passing test and a failing-input test.

### T-035 — Implement server-manager crate [CP]

- [ ] **Prerequisites:** T-030
- **Scope:** Artifact acquisition from the official channel recorded by T-023/T-024, verification before execution (SRV-002), generated configuration with RCON and query off **where the game has them, Java edition only** (SRV-004; Bedrock has neither, `research.md` RQ-13.3), Bedrock `enable-lan-visibility` off so a non-default port is not shadowed, `online-mode` left on, per-user data directory layout (SRV-010), process supervision (start, graceful stop, bounded crash restart, log capture), world backup and restore (SRV-008), and a provisionable-games list driven by the profiles crate so an unverified game cannot be provisioned (SRV-003). Bedrock has no `/reload`, so every configuration write must be sequenced stop → write → start. **Per ADR-015: do not enable RCON, generate no RCON credential, and treat the Server Management Protocol as opt-in and off by default; if enabled, bind it to localhost with a static port, generate its TLS keystore, and pass the keystore password by environment variable rather than writing it to disk (`research.md` RQ-13.4).** **Do not** accept a local target from any network message; the local port is passed in-process from the engine's own configuration (SRV-005, `contracts.md` §13).
- **Deliverables:** `crates/server-manager` with unit tests; dependency register entries for anything it adds.
- **Requirements:** SRV-002, SRV-003, SRV-004, SRV-008, SRV-010
- **Verification:** Unit tests including tampered-artifact and malformed-config cases
- **Done when:** A tampered artifact is rejected before execution; a generated configuration has RCON and query off; an unverified game is not in the provisionable list; no code path writes outside the per-user directory or requires elevation.


## M3 — Authentication, authorization, and control-plane API

*Gate G3: all routes in `api.md` implemented with schema-validated tests (admin routes covered by T-049, the last task of the milestone).*

### T-040 — Implement auth exchange, refresh, logout, session rotation

- [ ] **Prerequisites:** T-034, T-017
- **Scope:** Identity-provider integration (provider decision recorded first); refresh rotation with reuse detection.
- **Deliverables:** Auth module and tests.
- **Requirements:** SEC-001, SEC-003, FR-001
- **Verification:** Integration and security tests
- **Done when:** Reused refresh token revokes the family; wrong PKCE verifier and unlisted redirect URI are rejected.

### T-041 — Implement authorization policy and tenant isolation [CP]

- [ ] **Prerequisites:** T-034
- **Scope:** Central policy for the matrix in `contracts.md` §2; `not_found` for cross-owner access.
- **Deliverables:** Policy module and tests.
- **Requirements:** SEC-009
- **Verification:** Security tests with two users
- **Done when:** Cross-user reads and writes all return `not_found`.

### T-050 — Implement signing-key management and rotation

- [ ] **Prerequisites:** T-030
- **Scope:** Key generation, `kid` handling, overlap window rotation, relay verification key distribution.
- **Deliverables:** Key management module and runbook.
- **Requirements:** SEC-016, SEC-004
- **Verification:** Staging rotation exercise
- **Done when:** Rotation completes with tunnels connected; old keys rejected after overlap.

### T-042 — Implement tunnel create/list/get with idempotency [CP]

- [ ] **Prerequisites:** T-041, T-032
- **Scope:** Routes 6–8 of `api.md`; request validation against schemas; `idempotency_keys` handling.
- **Deliverables:** Routes and tests.
- **Requirements:** FR-004
- **Verification:** API tests against schemas
- **Done when:** Valid and invalid bodies behave per `contracts.md` §5.6; replay returns the same tunnel; same key with different body returns `idempotency_mismatch`.

### T-043 — Implement start, stop, token routes and relay allocation

- [ ] **Prerequisites:** T-042, T-050
- **Scope:** Routes 9–11; relay selection and port reservation; failover reallocation procedure.
- **Deliverables:** Routes, allocator, tests.
- **Requirements:** FR-005, FR-006, REL-002
- **Verification:** Integration tests
- **Done when:** Start returns a schema-valid response; stop releases ports; an unhealthy relay is skipped for new allocations.

### T-044 — Implement invitations, claim, and admissions

- [ ] **Prerequisites:** T-043, T-015
- **Scope:** Routes 12–16 and 18 (shape per T-015 decision); secret generation and hashing.
- **Deliverables:** Routes and tests.
- **Requirements:** FR-007, FR-008, FR-009, SEC-007
- **Verification:** API and security tests
- **Done when:** Secrets are never stored in plaintext or returned after creation; expired/revoked/exhausted all return `invitation_invalid`.

### T-045 — Implement rate limiting and quotas

- [ ] **Prerequisites:** T-043
- **Scope:** Limits for claim, refresh, and mutations; account quotas; suspension handling.
- **Deliverables:** Limiter module and config.
- **Requirements:** SEC-008, SEC-015
- **Verification:** Security tests
- **Done when:** Limits return `rate_limited`/`quota_exceeded`; a suspended user's tunnels are terminated.

### T-046 — Implement audit events and log redaction

- [ ] **Prerequisites:** T-043
- **Scope:** Audit writes for listed actions; allow-list for metadata; redaction in logging layer.
- **Deliverables:** Audit module and tests.
- **Requirements:** SEC-013, PRIV-002
- **Verification:** Leak test
- **Done when:** Events exist for each listed action; scan finds no tokens, payloads, or raw guest IPs.

### T-047 — Implement client API library with timeouts and retries [CP]

- [ ] **Prerequisites:** T-042
- **Scope:** Typed client used by the desktop app; retry and backoff per `contracts.md` §4.
- **Deliverables:** Client library and tests.
- **Requirements:** REL-003
- **Verification:** Unit tests with fault injection
- **Done when:** Retries occur only where specified and stop at the attempt limit.

### T-048 — Implement internal relay API (state and report)

- [ ] **Prerequisites:** T-043
- **Scope:** Routes 19–20; cursor-based state; usage upserts; relay restart recovery.
- **Deliverables:** Routes and tests.
- **Requirements:** REL-004
- **Verification:** Failure-injection test
- **Done when:** After simulated relay restart the state poll restores reservations and admissions.

### T-049 — Design and implement admin authentication and API

- [ ] **Prerequisites:** T-045, T-046
- **Scope:** Decide admin identity model (ADR); implement admin routes: suspend, terminate, relay drain.
- **Deliverables:** ADR; admin routes; tests.
- **Requirements:** SEC-018, SEC-015
- **Verification:** Design review; security tests
- **Done when:** Admin routes fail without admin authentication; all admin actions audited.


## M4 — Tunnel protocol, TCP, UDP, and relay

*Gate G4: TCP and UDP traffic flows end to end in the integration environment under tests.*

### T-060 — Relay skeleton: QUIC listener, hello, token validation

- [ ] **Prerequisites:** T-031, T-050
- **Scope:** Accept QUIC, validate token fields, replay cache, reject codes; no outbound dial path.
- **Deliverables:** `crates/relay` core and tests.
- **Requirements:** SEC-004, SEC-005, SEC-010
- **Verification:** Unit and security tests
- **Done when:** Each invalid-token case returns the specified `reject` code; code review confirms no client-directed outbound connect.

### T-061 — TCP forwarding path

- [ ] **Prerequisites:** T-060
- **Scope:** Public listener, per-connection stream, engine-side dial protocol (`0x00/0x01/0x02`), half-close, backpressure.
- **Deliverables:** Relay and engine TCP path.
- **Requirements:** NET-001, NET-003
- **Verification:** Network tests
- **Done when:** Hash-verified transfer both ways; works with inbound host connections blocked.

### T-062 — UDP forwarding path

- [ ] **Prerequisites:** T-060
- **Scope:** Flow table, datagram header, per-flow local sockets, oversize handling, flow expiry.
- **Deliverables:** Relay and engine UDP path.
- **Requirements:** NET-002, NET-007
- **Verification:** Network tests
- **Done when:** Two sources get only their own replies; oversize and unknown-index datagrams are dropped and counted.

### T-063 — Heartbeats, idle timeouts, resumption

- [ ] **Prerequisites:** T-061, T-062
- **Scope:** `ping`/`pong`, grace period, reconnect with same ports, TCP closure and UDP continuity on resume.
- **Deliverables:** Session management code.
- **Requirements:** NET-004, NET-005, FR-014
- **Verification:** Network tests with simulated outage
- **Done when:** Outage shorter than grace returns to Active; TCP connections closed; UDP flow resumes.

### T-064 — Limits and backpressure

- [ ] **Prerequisites:** T-061, T-062
- **Scope:** Flow caps, queue bounds, bandwidth limits, metrics for drops.
- **Deliverables:** Limit enforcement.
- **Requirements:** NET-006, OPS-004
- **Verification:** Flood test
- **Done when:** Over-limit load is refused or dropped and counted; relay memory stays bounded in the test.

### T-065 — Guest admission enforcement and revocation timing

- [ ] **Prerequisites:** T-044, T-061, T-062
- **Scope:** Relay admits only live admissions; state-poll integration; measure revocation latency.
- **Deliverables:** Enforcement code; latency measurement.
- **Requirements:** FR-009, PERF-007
- **Verification:** Integration test and measurement
- **Done when:** Unadmitted IP refused; revocation latency measured and recorded.

### T-066 — Fallback carrier (if T-016 found it viable)

- [ ] **Prerequisites:** T-016, T-063
- **Scope:** Implement chosen fallback; UI mode flag; document limits.
- **Deliverables:** Fallback transport.
- **Requirements:** NET-009
- **Verification:** Blocked-UDP network test
- **Done when:** Tunnel reaches Active with UDP to the relay blocked; reduced mode reported.

### T-067 — Graceful shutdown and resource cleanup

- [ ] **Prerequisites:** T-063
- **Scope:** Close sockets, release ports on stop and crash recovery; leak checks.
- **Deliverables:** Shutdown handling.
- **Requirements:** NET-008
- **Verification:** Port-scan after stop
- **Done when:** No listener remains for the tunnel after stop or recovery.


## M5 — Engine and client integration

*Gate G5: engine drives full tunnel lifecycle against the staging stack.*

### T-070 — Engine core: session state machine and API integration [CP]

- [ ] **Prerequisites:** T-047, T-063
- **Scope:** Implement `contracts.md` §12 engine state machine; connect to control plane and relay.
- **Deliverables:** `crates/engine` lifecycle.
- **Requirements:** FR-005, TEST-002
- **Verification:** Integration test
- **Done when:** Engine reaches Active against the integration stack; transitions match the table.

### T-071 — Local target policy and dialing [CP]

- [ ] **Prerequisites:** T-070, T-032
- **Scope:** Implement policy from `contracts.md` §13; refuse unknown mapping indexes; sensitive-port deny-list.
- **Deliverables:** Policy code and tests.
- **Requirements:** FR-003, SEC-006
- **Verification:** Unit and security tests
- **Done when:** Policy-violating targets cannot be configured; relay-supplied data cannot change targets.

### T-072 — Local reachability probes [CP]

- [ ] **Prerequisites:** T-071
- **Scope:** TCP connect probe; profile-defined UDP probes; `unknown` when no probe exists.
- **Deliverables:** Probe module.
- **Requirements:** FR-011
- **Verification:** Tests with listening and stopped servers
- **Done when:** Correct results for both; UDP never reports reachable without a response.

### T-073 — Players-seen from flow summaries

- [ ] **Prerequisites:** T-070
- **Scope:** Consume `flow_summary`; expose count labeled as observed traffic.
- **Deliverables:** Engine events.
- **Requirements:** FR-012
- **Verification:** Integration test with scripted guests
- **Done when:** Count equals the number of scripted guests.

### T-074 — Credential-store integration

- [ ] **Prerequisites:** T-017, T-070
- **Scope:** Store and retrieve refresh token via the OS credential store; no secrets in files.
- **Deliverables:** Credential module.
- **Requirements:** SEC-002
- **Verification:** Scan of files, logs, dumps
- **Done when:** Scan finds no secret strings.

### T-075 — Diagnostics bundle with redaction

- [ ] **Prerequisites:** T-046, T-070
- **Scope:** Export logs and status; redaction pipeline.
- **Deliverables:** Diagnostics export.
- **Requirements:** FR-013, PRIV-002
- **Verification:** Automated scan
- **Done when:** No tokens, raw guest IPs, or payload bytes in the bundle.

### T-076 — Managed server lifecycle integration with engine and UI events

- [ ] **Prerequisites:** T-070, T-035
- **Scope:** Wire `crates/server-manager` into the engine lifecycle; push managed server state to the UI as its own indicator (SRV-006) without merging it with tunnel, local-server, or players-seen state; keep the managed server's address and port in-process and prove no network message carries them (SRV-005); stop the server when the tunnel stops and when the app exits (SRV-007). If the Server Management Protocol is enabled for a managed Java server, surface its player join/leave notifications as a **separate confirmed-presence signal** that never merges with the traffic count (SRV-011, `research.md` RQ-13.4).
- **Deliverables:** Engine integration and events per `contracts.md` §13.
- **Requirements:** SRV-005, SRV-006, SRV-007
- **Verification:** Integration test with request capture; UI state-independence test
- **Done when:** A captured provisioning and tunnel-start sequence contains no managed-server address or port; the four indicators change independently; no server process survives app exit.


## M6 — Desktop UI and onboarding

*Gate G6: first-tunnel flow works end to end through the UI.*

### T-080 — UI shell and design tokens

- [ ] **Prerequisites:** T-018, T-070
- **Scope:** Implement tokens and base components from `DESIGN.md`; theme support per its decision.
- **Deliverables:** `apps/desktop` shell.
- **Requirements:** UX-005, UX-007
- **Verification:** Automated contrast check
- **Done when:** All defined token pairs pass the contrast thresholds in `DESIGN.md`.

### T-081 — Onboarding and sign-in screens

- [ ] **Prerequisites:** T-080, T-040, T-074
- **Scope:** First-launch flow; system-browser sign-in; sign-out.
- **Deliverables:** Screens.
- **Requirements:** FR-001, UX-001
- **Verification:** UI test and manual run
- **Done when:** A fresh install reaches a signed-in state using only the browser.

### T-082 — Game selection and local configuration screens [CP]

- [ ] **Prerequisites:** T-080, T-071, T-072
- **Scope:** Profile list; per-mapping local target entry; validation messages; custom ports.
- **Deliverables:** Screens.
- **Requirements:** FR-002, FR-003, GAME-009
- **Verification:** UI tests
- **Done when:** All five profiles listed; invalid targets rejected with messages; non-default ports forward in an L0 check.

### T-083 — Tunnel dashboard with three-layer status [CP]

- [ ] **Prerequisites:** T-082, T-073
- **Scope:** Separate indicators for tunnel, local server, players seen; stop confirmation.
- **Deliverables:** Dashboard.
- **Requirements:** FR-010, UX-002, UX-006
- **Verification:** UI tests driving each indicator
- **Done when:** Each indicator changes independently; stop confirmation appears only with players seen.

### T-084 — Invitation and sharing screens [CP]

- [ ] **Prerequisites:** T-083, T-044
- **Scope:** Create, copy, list, revoke invitations; guest claim page.
- **Deliverables:** Screens and guest page.
- **Requirements:** FR-007, FR-008, FR-009
- **Verification:** UI and integration tests
- **Done when:** Secret shown once; revocation reflected in list.

### T-085 — Errors and troubleshooting guidance

- [ ] **Prerequisites:** T-083
- **Scope:** Map every error code and engine error to cause and action; troubleshooting flows.
- **Deliverables:** Message catalog.
- **Requirements:** FR-015, UX-003
- **Verification:** Review and UI tests
- **Done when:** Every code in `contracts.md` §3 has a reviewed message.

### T-086 — Accessibility pass

- [ ] **Prerequisites:** T-084, T-085
- **Scope:** Keyboard operation, focus visibility, screen reader labels.
- **Deliverables:** Fixes and test script.
- **Requirements:** UX-004
- **Verification:** Keyboard-only and screen-reader runs
- **Done when:** Onboarding, start, share, stop completed by keyboard only.

### T-087 — Usability study round 1

- [ ] **Prerequisites:** T-086
- **Scope:** Scripted first-tunnel sessions with participants; record success and time.
- **Deliverables:** Study report; targets for UX-001.
- **Requirements:** UX-001
- **Verification:** Study
- **Done when:** Report has results; a target is recorded from the first round.

### T-088 — Managed server screens [CP]

- [ ] **Prerequisites:** T-082, T-076
- **Scope:** On the local-server screen, offer "Set this up for me" only for profiles the provisionable list allows (SRV-003). Show provisioning progress with a cancel option, the managed server's own status indicator (SRV-006), download and verification progress, crash and log excerpt, world backup/restore, and update with confirmation while a tunnel is active (SRV-009). Per-game guidance on what a host still has to decide (world settings, EULA acceptance). Manual configuration must remain available and equally supported.
- **Deliverables:** Screens; message catalog entries for the new engine errors.
- **Requirements:** SRV-001, SRV-003, SRV-006, SRV-009
- **Verification:** UI tests driving each managed-server state
- **Done when:** Every managed server state has a distinct, reviewed message with a cause and an action; manual configuration still works for all five profiles; a host can provision, update, and stop without editing a file by hand.


## M7 — Automated testing, compatibility, security, performance

*Gate G7: compatibility, security, network, performance, and failure-injection tests pass; compatibility results recorded per game. (Privacy tests run in M8 after retention jobs exist: T-098.)*

### T-090 — Game compatibility automation (L0 and L1)

- [ ] **Prerequisites:** T-072, T-062, T-032
- **Scope:** Containerized servers where available; Minecraft status probes; custom probes for others.
- **Deliverables:** `tests/game-compat`.
- **Requirements:** TEST-005, GAME-001, GAME-002
- **Verification:** CI job
- **Done when:** L0/L1 pass for the games with tooling; gaps are listed explicitly.

### T-091 — L2 real-client compatibility runs per game [CP]

- [ ] **Prerequisites:** T-090, T-084
- **Scope:** Manual runs with real clients for all five games; results recorded per release.
- **Deliverables:** Compatibility report.
- **Requirements:** GAME-001, GAME-002, GAME-003, GAME-004, GAME-005, GAME-006
- **Verification:** Manual L2 with recorded evidence
- **Done when:** Each game has a recorded L2 result for the release candidate; failing games are marked unsupported in UI and docs.

### T-092 — Network impairment harness and IPv6 cases

- [ ] **Prerequisites:** T-063
- **Scope:** Loss, jitter, latency, blocked UDP, restricted NAT; IPv4/IPv6 cases.
- **Deliverables:** `tests/network`.
- **Requirements:** TEST-003, NET-005, NET-010
- **Verification:** CI or lab run
- **Done when:** Each scenario asserts the documented behavior.

### T-093 — Security test suite

- [ ] **Prerequisites:** T-065, T-049, T-046
- **Scope:** Tests for unauthorized access, expired tokens, isolation, destination validation, port exposure, rate limits, secret leakage, resource exhaustion.
- **Deliverables:** `tests/security`.
- **Requirements:** TEST-004, SEC-002, SEC-005, SEC-006, SEC-009, SEC-010, SEC-013, SEC-008
- **Verification:** CI
- **Done when:** Every category passes; any failure blocks merge.

### T-094 — Fuzzing of protocol inputs

- [ ] **Prerequisites:** T-031, T-062
- **Scope:** Fuzz message parsers and datagram handling.
- **Deliverables:** Fuzz targets and results.
- **Requirements:** NET-007
- **Verification:** Fuzz run
- **Done when:** No panic, hang, or cross-session effect in the recorded run.

### T-095 — Benchmark harness and baselines

- [ ] **Prerequisites:** T-061, T-062
- **Scope:** Reproducible harness; latency, throughput, CPU, memory, connection time; select targets from baselines.
- **Deliverables:** `tests/perf`, report, ADR for targets.
- **Requirements:** TEST-006, PERF-001, PERF-002, PERF-003, PERF-004
- **Verification:** Benchmark run
- **Done when:** Reports exist; targets recorded in `research.md` ADR before any claim.

### T-096 — Relay load test and capacity limits

- [ ] **Prerequisites:** T-064, T-095
- **Scope:** Concurrent session and flow load; find saturation; set `max_allocations` and caps.
- **Deliverables:** Load report; configured limits.
- **Requirements:** PERF-005, OPS-004, NET-006
- **Verification:** Load test
- **Done when:** Saturation behavior documented; limits configured from results.

### T-097 — Failure-injection tests

- [ ] **Prerequisites:** T-048, T-043
- **Scope:** Stop control plane; kill relay; verify behavior.
- **Deliverables:** Test suite.
- **Requirements:** REL-001, REL-002, REL-004
- **Verification:** Failure-injection run
- **Done when:** Tunnels survive control-plane outage until credential expiry; unhealthy relay avoided.


## M8 — Packaging, observability, deployment, operations

*Gate G8: staging environment passes exercises for deploy, rollback, backup, alerts, and updates.*

### T-100 — Per-user installer and WebView2 handling

- [ ] **Prerequisites:** T-019, T-080
- **Scope:** Build installer; detect or install WebView2; standard-user operation.
- **Deliverables:** Installer.
- **Requirements:** INST-001, INST-005, SEC-012
- **Verification:** Clean standard-user VM run
- **Done when:** Install, run, uninstall succeed without administrator rights; missing WebView2 handled.

### T-101 — Updater integration, signing, rollback

- [ ] **Prerequisites:** T-100
- **Scope:** Signed update pipeline; verify; failed-update recovery.
- **Deliverables:** Updater and signing process.
- **Requirements:** INST-002, INST-003, SEC-014
- **Verification:** Staging exercise
- **Done when:** Tampered artifact rejected; interrupted update leaves old version working.

### T-102 — Uninstall behavior

- [ ] **Prerequisites:** T-100
- **Scope:** Remove processes and listeners; offer data removal.
- **Deliverables:** Uninstaller behavior.
- **Requirements:** INST-004
- **Verification:** Manual VM test
- **Done when:** No app processes or listeners remain after uninstall.

### T-103 — Observability: metrics, logs, alerts, cost dashboard

- [ ] **Prerequisites:** T-064, T-048
- **Scope:** Instrument components; alert rules; egress cost dashboard.
- **Deliverables:** Dashboards and alerts.
- **Requirements:** OPS-002, OPS-006
- **Verification:** Staging exercise
- **Done when:** Alerts fire for relay unhealthy, allocation failures, saturation; cost view shows egress.

### T-104 — Environments and infrastructure definitions

- [ ] **Prerequisites:** T-030
- **Scope:** Dev, staging, production definitions; separate secrets and signing keys.
- **Deliverables:** `infra/`.
- **Requirements:** OPS-001
- **Verification:** Infrastructure review
- **Done when:** No production secret is present in staging.

### T-105 — Deployment and rollback rehearsal

- [ ] **Prerequisites:** T-104, T-103
- **Scope:** Automated deploy; rollback; timing.
- **Deliverables:** Runbook.
- **Requirements:** OPS-003
- **Verification:** Staging rollback exercise
- **Done when:** Rollback performed successfully and timed.

### T-106 — Backup and restore test

- [ ] **Prerequisites:** T-104
- **Scope:** Schedule backups; restore to clean DB; smoke test.
- **Deliverables:** Backup procedure.
- **Requirements:** OPS-007
- **Verification:** Restore exercise
- **Done when:** Restored database passes smoke test.

### T-107 — DDoS and abuse mitigation plan

- [ ] **Prerequisites:** T-096
- **Scope:** Research provider-level protections from provider documentation; document controls and response steps.
- **Deliverables:** Plan document.
- **Requirements:** OPS-005
- **Verification:** Document review
- **Done when:** Plan cites provider documentation and lists response steps.

### T-108 — Incident runbook and tabletop exercise

- [ ] **Prerequisites:** T-049, T-103
- **Scope:** Runbook for detection, containment, key rotation, communication; one tabletop.
- **Deliverables:** Runbook; exercise notes.
- **Requirements:** OPS-008, SEC-015
- **Verification:** Tabletop
- **Done when:** Exercise completed and findings recorded.

### T-109 — Retention configuration and cleanup jobs

- [ ] **Prerequisites:** T-034
- **Scope:** Set retention values in requirements; implement scheduled deletions.
- **Deliverables:** Jobs; updated `requirements.md` values.
- **Requirements:** PRIV-003
- **Verification:** Integration test
- **Done when:** Expired rows removed by the job in test.

### T-098 — Privacy tests

- [ ] **Prerequisites:** T-046, T-109
- **Scope:** Default-install network capture; retention job tests; account deletion test.
- **Deliverables:** Test suite.
- **Requirements:** PRIV-001, PRIV-003, PRIV-004
- **Verification:** Tests
- **Done when:** Capture shows only needed endpoints; expired rows removed; deletion removes user data.

### T-110 — Privacy notice and encryption disclosure

- [ ] **Prerequisites:** T-022
- **Scope:** Write notice stating relay visibility, metadata, retention; in-app text.
- **Deliverables:** Notice and UI text.
- **Requirements:** PRIV-005, SEC-011
- **Verification:** Document and UI review
- **Done when:** Notice matches actual behavior listed in `architecture.md` §18.


## M9 — Release readiness

*Gate G9: go/no-go review.*

### T-120 — Cross-document consistency and traceability audit [CP]

- [ ] **Prerequisites:** T-098, T-091, T-093
- **Scope:** Run audit: requirement-to-task mapping, task-to-requirement links, API/data-model agreement, diagram agreement.
- **Deliverables:** Audit report.
- **Requirements:** TEST-008
- **Verification:** Audit script and review
- **Done when:** Report shows zero unmapped requirements and zero unresolved contradictions.

### T-121 — Release checklist and go/no-go [CP]

- [ ] **Prerequisites:** T-120, T-101, T-105
- **Scope:** Run release checklist; confirm direct-connect disabled; confirm support claims match L2 results.
- **Deliverables:** Signed-off checklist.
- **Requirements:** TEST-008, GAME-006, NET-011, TEST-005
- **Verification:** Checklist review
- **Done when:** All P0 requirements verified; unsupported items stated in docs and UI.

### T-122 — Update README with verified commands [CP]

- [ ] **Prerequisites:** T-121
- **Scope:** Replace placeholders with commands that were actually executed.
- **Deliverables:** Updated `README.md`.
- **Requirements:** TEST-008
- **Verification:** Run each documented command in a clean environment
- **Done when:** Every command in README succeeds on a clean checkout.


## Critical path

T-001 → T-002 → T-004 → T-010 → T-012 → T-020 → T-022 → T-030 → T-033 → T-034 → T-041 → T-042 → T-047 → T-070 → T-071 → T-072 → T-082 → T-083 → T-084 → T-091 → T-120 → T-121 → T-122

This is the longest chain of prerequisites, computed by `scripts/audit_traceability.py` on 2026-10-17 — **not** by hand, unlike earlier versions of this note. It is **23 tasks** long. **No effort or duration estimates exist yet**, so this shows structural dependency depth, not calendar time. Tasks outside this chain can still block a release if they slip.

**Recomputed 2026-10-17 after adding T-023 to T-027 as prerequisites of T-022.** The chain length is unchanged at 23 tasks, and the chain above is still one of the longest. Two further chains of the same length now exist, through the new provisioning experiments:

- via the game join experiment: T-001 → T-002 → T-004 → T-010 → T-013 → T-014 → T-022 → … (same tail)
- via the provisioning experiments: T-001 → T-002 → T-004 → T-023 → T-025 → T-026 → T-022 → … (same tail)

**This chain length is machine-computed** as of 2026-10-17. The script resolves ties alphabetically, so where several chains share the maximum depth (see below) it reports one deterministically; the count of 23 is the part that matters and it is now verified rather than hand-derived.

**Known forward references (all confirmed benign, reported by the audit as warnings):** T-022 → T-023…T-027 (introduced when the provisioning experiments were added as prerequisites of gate G1), and two pre-existing ones: **T-043 → T-050** (start/stop needs signing-key management) and **T-098 → T-109** (privacy tests need the retention jobs). The last two predate this work and were found by the audit, not introduced by it. Per rule 2, the Prerequisites field is authoritative, so no renumbering is required; renumbering T-022 would have meant editing 14 references across four files for cosmetic ordering.

T-035, T-076, and T-088 are **off** the longest chain, so this feature does not extend it. This was computed by hand from the prerequisite fields; T-120 must confirm it with the audit script.
