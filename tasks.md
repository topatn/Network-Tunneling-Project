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

- [x] **Prerequisites:** none
- **Scope:** Initialize repository, workspace layout from `architecture.md` §4, formatting and lint config, empty CI pipeline with a dependency-scan step; place `README.md`, `AGENTS.md`, and `CLAUDE.md` at the repository root and fix their relative links.
- **Deliverables:** Repo, CI config, CONTRIBUTING pointer to `AGENTS.md`.
- **Requirements:** TEST-007, SEC-017
- **Verification:** CI run on an empty commit
- **Progress 2026-10-17 — ALMOST complete. Two conditions still unmet.** Installed `rustc 1.99.1` via rustup (both `x86_64-pc-windows-msvc` and `x86_64-pc-windows-gnu`). `git init` and two commits made. Directory skeleton per `architecture.md` §4; root `Cargo.toml` workspace plus six crate manifests with deliberately **empty** `[dependencies]`; six `src/lib.rs` crate roots; `rustfmt.toml`, `clippy.toml`, `deny.toml`, `.gitignore`, `.gitattributes`; `.github/workflows/ci.yml`; `CONTRIBUTING.md`; `docs/dependency-register.md`; `scripts/audit_traceability.py`; README/AGENTS/CLAUDE confirmed at repository root. **Verified by running it** (`rustc 1.99.1`): `cargo metadata`, `cargo check`, `cargo fmt --all -- --check`, `cargo clippy -- -D warnings` (clean), `cargo test` under the GNU toolchain (0 tests), and **`cargo deny check advisories bans licenses` using the official prebuilt cargo-deny 0.20.2 Windows binary with its published SHA-256 verified**. **Three gates were each proven to fail when they should:** the lint gate denies `unwrap()` (exit 101), the dependency gate reports `RUSTSEC-2020-0071` on a seeded `time 0.1.45` (exit 1), and the traceability audit detects 5 of 5 planted defects. **Three configuration bugs were caught only by running the checks** — an invalid rustc lint in `clippy.toml`, two non-existent clippy lint names that would have silently enforced nothing, and a `deny.toml` that had never been parsed and used a key removed in cargo-deny 0.20. **Progress 2026-10-18 — condition (1) is now MET; the box stays unchecked on (2) alone.** A Git remote was configured and, for the first time, ci.yml ran green on hosted infrastructure: **six jobs passed** — Format, Lint, **Test** (on windows-latest, the first time Rust was compiled in this project with a complete toolchain rather than the local GNU bundle's ld.exe), Documentation traceability audit, Design token contrast, and Dependency scan (which installed cargo-deny from source on the runner rather than using the prebuilt binary used locally). clean-vm.yml also ran green. **Merges are now actually gated**, which was the whole point of the condition. **One condition remains, so this box is still unchecked:** the migration-apply job is disabled (if: false) until T-033 creates migrations/ — it shows as *skipped*, not passed. **Environment gaps that remain** (not blocking T-001's gates, but blocking other work): no Windows SDK, so MSVC cannot link, and the GNU toolchain has no `dlltool.exe`, so neither toolchain can compile Rust tooling from source.

**Recorded inconsistency (decide at T-004):** `architecture.md` §4 describes `docs/` as holding the documentation set, but the 20 documents live at the repository root and only `docs/dependency-register.md` is under `docs/`. Not changed here — moving 20 files would break every relative link and is outside T-001's scope.
- **Done when:** CI runs format, lint, and dependency scan steps and passes on the skeleton; a seeded known-vulnerable test dependency makes the scan fail.
  - **Condition (2) met on 2026-10-18 — box ticked.** The migration-apply job was the last open item: it was `if: false` because no migration tool had been chosen and `migrations/` did not exist, so it showed as *skipped* rather than passed. T-033 supplied both, the job was enabled, and **CI run 10ac6f4 passed all 8 jobs including both migration matrix legs**. With that, both Done-when conditions are demonstrated by their stated verification. **The environment gaps recorded above still stand and are not resolved by this:** no Windows SDK, so MSVC cannot link, and no `dlltool.exe` in the GNU bundle, so neither toolchain can compile Rust tooling from source. Those blocked T-003's local path and are worked around on the CI runner instead.

### T-002 — Replace reported game facts with primary sources [CP]

- [ ] **Prerequisites:** T-001
- **Scope:** Read Mojang/minecraft.wiki and Valve documentation; upgrade or correct every `[R]` item in `research.md` §2; update GAME requirement notes if facts change.
- **Deliverables:** Updated `research.md`; change list.
- **Requirements:** GAME-006
- **Verification:** Reviewer checks each §2 row cites a primary source or is still labeled `[R]`
- **Done when:** Every game row is either cited to a primary source or explicitly still `[R]` with a linked experiment.
- **Progress 2026-10-17 — PARTIAL. NOT complete.** Read this session: Minecraft Wiki `server.properties` (the shipped default files for Java 26.3 and BDS 1.26.52.3, both editions, in full), minecraft.wiki "Server/Requirements", "Server.jar", "Version_manifest.json", "Bedrock Dedicated Server", "Minecraft Server Management Protocol"; Microsoft Learn BDS getting-started and BDS properties; Terraria Wiki "Server"; Left4DevOps `l4d2-docker`. **Upgraded to `[V]`:** Minecraft Java `server-port` TCP 25565, `query.port` UDP 25565, `rcon.port` TCP 25575 and the `enable-rcon=false` / `enable-query=false` defaults; Minecraft Bedrock 19132/19133 and its property set (two independent sources); Terraria TCP 7777. **Added §2.1** (server defaults that change our security posture — Terraria's `upnp=1` auto-port-forward is the headline; also JMX, `server-udp-ports`, and Minecraft's `prevent-proxy-connections`) and **§2.3** (Terraria's UDP question, with a reading that the TCP/UDP mentions are defensive forwarding advice rather than a UDP listener — explicitly left `[X]`). **§2.2 is the important new finding:** Source-engine games identify players **by IP**, and because our engine dials the game server from one local address, all guests may collapse into a single identity and the game's own RCON brute-force ban could lock the host out. Raised as `AS-12`, an `architecture.md` open issue (13, high priority), and a threat-model row. **Corrected my own earlier error:** I had written that Bedrock's `server-udp-ports` did not exist; it does, in the shipped file, and the original `[R]` claim was sound. **Still `[R]`, so NOT done:** Terraria's TCP-vs-UDP contradiction (E-1); **all Valve/Source `[V]` rows could not be re-verified — `developer.valvesoftware.com` now returns an Anubis anti-bot interstitial**, so those labels rest on the 2026-10-16 reading; and a Bedrock `allow-list` default conflict between two sources. Nothing was executed against a running server.

### T-003 — Choose migration tool and verify DDL on real PostgreSQL [CP]

- [x] **Prerequisites:** T-001
- **Scope:** Select a migration tool (record as ADR); run `schema-draft.sql` on real PostgreSQL versions; record supported versions and any fixes.
- **Deliverables:** ADR; corrected `schema-draft.sql`; version matrix.
- **Requirements:** TEST-007, PRIV-006
- **Verification:** Run DDL on each target version
- **Done when:** DDL applies cleanly to an empty database on every version in the matrix; no email column exists.
- **Environment blocker, recorded 2026-10-17.** Compiling Rust tooling from source is impossible on this machine and **cannot be fixed from an unelevated session**: the MSVC toolchain has no Windows SDK, and the GNU toolchain's self-contained MinGW bundle ships no assembler, so `dlltool` cannot spawn one. `cargo install sqlx-cli` therefore fails, which is why ADR-017's chosen migration tool has never been executed. Fixing it needs elevation — the Windows SDK (~1–2 GB), MinGW-w64 (~100 MB), or building on a Linux/CI runner. **Owner action.** Details in `docs/dependency-register.md`.
- **Progress 2026-10-17 — SUBSTANTIALLY COMPLETE. Two parts still open.** **ADR-008 was Accepted on 2026-10-17** (Rust control plane with `axum`, PostgreSQL via `sqlx`); consequences recorded: the workspace MSRV rises **1.77 → 1.94**, and the control plane gains a 145-crate dependency tree for T-030 to register. **The DDL has now been executed for real**, on official EDB standalone binaries extracted to a temporary directory — no system install, no service, no registry change. Results on **PostgreSQL 16.10 and 17.6**: the schema applies cleanly to an empty database with **zero errors**, producing **12 tables, 35 indexes, 57 constraints, 13 foreign keys** identically on both; **PRIV-006 verified** (0 columns matching `%email%`); **INV-1 verified** (no local address/port/target column; the only `%port%` columns are the relay's own public ports, which is the distinction INV-1 draws); `audit_events` confirmed to have 0 foreign keys; and four negative tests rejected by `users_status_chk`, `tunnels_profile_version_chk`, `tunnels_expiry_chk`, `tunnels_stopped_consistency_chk`. `gen_random_uuid()` needs no extension on either version. Full matrix and method in `schema.md`. **Migration tool decided: `sqlx migrate`** (ADR-017, accepted 2026-10-17). The driver is already `sqlx` under ADR-008, so this adds **no further dependency**; `refinery` would have meant a second migration library, and hand-rolled SQL was rejected because `data-model.md` §8's "never edit an applied migration" rule is best enforced by a tool that tracks applied checksums. **`sqlx-cli` could not be executed here** — `dlltool.exe` is missing from the GNU toolchain, MSVC has no Windows SDK, and no prebuilt binary is published — so T-033 must verify it rather than assume it. **Still open, and why the box stays unchecked:** (1) **verification ran through `postgres --single`, not a normal client connection**, because forked backends crash in this environment with `STATUS_DLL_INIT_FAILED` — so concurrency, isolation, and connection handling are untested, and T-033's CI job must provide that; (2) the chosen tool itself has never been run, so its statement-boundary parsing in particular is unconfirmed — the exact defect class that produced a zero-table schema during the manual verification. A harness defect was found and fixed on the way: line-oriented submission split multi-line `CREATE TABLE`s, which would silently do the same in any line-based runner.
  - **Final blocker cleared on 2026-10-18 — box ticked.** The remaining item was that verification had only ever run through `postgres --single`, because forked backends crash in this environment with `STATUS_DLL_INIT_FAILED`, so concurrency, isolation and connection handling were untested. **CI run 10ac6f4 runs the whole migration through a normal client connection against PostgreSQL 16 and 17 service containers and passes**, which is the verification this task asked for. Every claim in the version matrix above is now backed either by the local `--single` apply or by that run. **The local limitation itself is unchanged and still recorded:** nothing here proves concurrency or isolation under load, and `sqlx-cli` still cannot be built on this machine. Do not read this tick as the environment being fixed.

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
- **Added 2026-10-17 — the per-guest loopback question is now narrowed to one thing.** `research.md` §2.2.1 tested the candidate fix and Windows preserves a distinct loopback source per flow for TCP and UDP, 64-way concurrent, no elevation. **What only a real server can answer, and is the point of this task:** does `srcds_run` treat `127.0.0.2` as a distinct player, does it accept loopback sources other than `127.0.0.1`, and does a failed RCON login ban *that* address rather than the host's. Record all three; do not infer them from the OS result.

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
- **`scripts/verify_clean_vm.py` exists to make that verification one command** (added 2026-10-17). It runs the whole acquisition path — `SRV-018` runtime selection, the `SRV-002` integrity chain, `--initSettings`, `SRV-004` default checks — then **stops at the EULA gate with exit code 10**, because `SRV-015` forbids accepting it for a host. The operator sets `eula=true` by hand and re-runs `--run`, which starts the server, waits for the readiness line, probes `127.0.0.1` and `::1` (`FR-011`), stops gracefully over the stdin pipe (`SRV-007`), and reports footprint and side effects. **Proven on this developer machine: 11/11 checks on the host-JRE branch and 12/12 on the fetched-JRE branch (`--simulate-no-host-java`), both exit 0** — neither of which is a clean VM, so **the box stays unchecked**. **The clean-VM run needs a Windows Pro, Enterprise, or Education machine** (`research.md` §5 RQ-13.2f): this host is Windows 10 Home Single Language, which does not have Windows Sandbox at all, and the Hyper-V and optional-feature queries need elevation. **`clean-vm.yml` was added 2026-10-17 to run this on a hosted Windows runner instead**, a fresh image with nothing preinstalled — manual dispatch only, with the EULA acceptance behind an explicit `accept_eula` input that defaults to false. **It RAN GREEN on 2026-10-17 — the first workflow in this repository to execute on hosted infrastructure.** Phase 1 passed every check on a runner that genuinely has **no Java installed**, so the `SRV-018` fetch branch ran for real: SHA-256 verified, extracted per-user, `java -version` re-read as 25, then the full `SRV-002` SHA-1 chain, `--initSettings`, the three `SRV-004` defaults, and a clean stop at the `SRV-015` EULA gate. Two workflow defects were found by that run and fixed. **The box still stays unchecked:** phase 2 needs `accept_eula`, and even a full green result would fall short of `SRV-001`, which names a *standard user* while the runner account is an administrator (`research.md` §5 RQ-13.2g). Three bugs in the script were caught by running it and are recorded in the code: a legacy `1.8` version parsed as Java 1, a documented `--run` standalone mode that raised `KeyError`, and a fallback that compared candidates against a required version of **0** — which would have accepted Java 8 for a server needing 25, the exact failure `SRV-018` exists to prevent.
- **Progress 2026-10-17 — the integrity question is now ANSWERED and verified end to end. Box still unchecked.** The two `[X]` blockers are closed. **The EULA and Usage Guidelines were read** (`research.md` §5 RQ-13.1b) — hosting is explicitly permitted and six binding obligations became `SRV-012`–`SRV-017`. **The real manifest was fetched and a checksum verified end to end** (`research.md` §5 RQ-13.2b): `version_manifest_v2.json` returned HTTP 200 / 277,821 bytes / 919 versions, latest release `26.3`; the manifest entry's `sha1` was proven to cover **the version JSON and not the server jar**, and the correct hash — `downloads.server.sha1` = `33680f5f…5f4c` — verified exactly against a real 62,294,556-byte download. **There is no sidecar `.sha1`/`.sha256`** (both 404), so the version JSON is the only source of the expected hash. **Unattended download works** with plain HTTPS, no cookies and no auth. **The required runtime is Java 25**, read from the version JSON per version rather than hard-coded, and this host already has 25.0.1 LTS. **The artifact is a self-contained bundler jar** carrying its own 40 dependency libraries, so no separate library resolution is needed. **A correction to my own work is recorded rather than hidden:** my first download script truncated the file on every chunk and I briefly concluded from the garbage that the jar was encrypted; it was my bug, the hash had been valid throughout, and a correctly downloaded file starts with ZIP magic. **Start-up verified the same day with the owner's authorisation** (`research.md` §5 RQ-13.2c): `--initSettings` wrote `server.properties` and an `eula=false` file and exited 0; the server **refused to start** until the EULA was accepted — **and exited 0 when refusing**, which is now a `SRV-007` requirement because a supervisor would otherwise read it as a clean start; after acceptance it reached **"Done (1.320s)!"** and listened on `*:25565`; `FR-011`'s probe reached both `127.0.0.1` and `::1`; a distinct loopback source `127.0.0.7` also connected, confirming §2.2.1's mechanism against a real server; `enable-rcon` and `enable-query` were both `false` and `prevent-proxy-connections` was `false` as §2.2 requires; no elevation, service, or scheduled task; footprint **125.8 MB across 87 files**. **Two supervision facts nobody had anticipated:** a forced kill saves nothing, so the engine must hold a writable stdin pipe and write `stop`, and **two JVM processes** appear with only one owning the socket, so it must track the process tree. `ADR-015`'s behaviour was observed — the server generates its own management secret — **and RQ-13.4's recorded length of 40 characters is wrong; it is 43.** **Still required, so the box stays unchecked:** (1) **whether a typical host can reach the runtime vendor's API and its CDN** — `research.md` §5 RQ-13.2d verified the fetch and checksum from here, and Mojang's own runtime manifest could not be located at all after 36 probes; (2) the clean-VM footprint check; (3) the MSMP documentation. **The JRE question that was open is now answered:** a per-user JRE 25 ZIP from a documented API, SHA-256 verified, needs no elevation and **ran the server to "Done (1.651s)!" with a stripped `PATH` and no `JAVA_HOME`**, so the "host has no Java" case is solved. Footprint budget is therefore **~307 MB / 400 files** per server. **The refusal path is now specified as `SRV-018`** after discovering that this machine carries **both** Java 25 and a **Java 8** runtime, and that naive discovery can land on the Java 8 and crash with `UnsupportedClassVersionError … class file version 69.0 … only recognizes up to 52.0` (`research.md` §5 RQ-13.2e).
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
- **Progress 2026-10-17 — licensing half DONE; VM half still not started, so the box stays unchecked.** The EULA and Usage Guidelines were **read in full** after repeated `webfetch` timeouts: a direct request confirmed DNS and TCP 443 were reachable all along, and `aka.ms/MinecraftEULA` returned **HTTP 200, 35,444 bytes** (redirects to `minecraft.net/en-us/eula`); `/en-us/usage-guidelines` returned **HTTP 200, 407,376 bytes**. Hosting is **explicitly permitted**, including charging for access, and building tools and services is permitted provided they do not appear official. **Six binding constraints were found and became requirements `SRV-012`–`SRV-017`** — including that guest access must be limited to genuine paid-for owners (no vendor API exists to verify this, so it is the host's obligation and we must not claim otherwise), and that access must **not** be gated on out-of-game content, which is new evidence against guest model G1. Full quotes and the three unresolved questions in `research.md` §5 RQ-13.1b. **This is not legal advice:** owner or counsel must still confirm our implementation complies. **Still not done here: the clean standard-user VM footprint check.** That half needs a real VM and has not been run.
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

- [x] **Prerequisites:** T-030
- **Progress 2026-10-18 — VERIFIED AND TICKED, with two ADR candidates open.**
  `crates/profiles` is a working loader; `profiles/` holds the five games. `serde`,
  `serde_json` and `uuid` are added to that crate, but **all three were already direct
  workspace dependencies**, so `Cargo.lock` gains five lines and **no new crate** — the
  count stays at the **167** measured for T-034. Both numbers were re-measured, not assumed.
  - **JSON over TOML**, which `tasks.md` allowed either way: `contracts.md`'s payloads are
    already JSON, and TOML would have been the workspace's first new dependency while T-030 —
    the gate task whose Done-when is that every dependency has a row — is still open.
    `registry.rs` is the only place a file is parsed, so changing it later is contained.
  - **The evidence label is load-bearing, and that is the whole design.** Each mapping declares
    `evidence: verified | reported`, and `enabled_by_default = !optional && evidence ==
    Verified` is **derived rather than stored**. `GAME-008` (optional mappings off by default)
    and `GAME-006` (no unverified claims) therefore have **no field to violate**, so neither
    needs a check to keep working. This is the T-034 lesson applied a second time: a structure
    that cannot express the violation beats an invariant that catches it.
  - **The consequence is product-visible and is the owner's call.** Terraria's UDP port is
    genuinely unresolved — its own source contradicts itself — and L4D1 is `[R]`
    throughout. So **`l4d1` ships exposing nothing by default.** It is not excluded: the
    profile loads, validates and lists, and a host who asks gets port 27015. Raised as
    **ADR-021**, a candidate, in `research.md` §6b with the alternatives and the named tests
    that would change.
  - **INV-1 is structural here.** Every profile type sets `deny_unknown_fields`, so there is no
    field in which a profile could name a local destination, and a `local_host` or `target`
    key is a **load error** rather than a silently ignored line. Two separate cases in the
    tests plant exactly those keys, because the first being rejected proves nothing about the
    second.
  - **Structural faults are enforced by types; only cross-field facts by the validator.**
    `Protocol` is an enum, so `"sctp"` is rejected by the deserializer; `default_port` is a
    `u16`, so `70000` never becomes a mapping. `validate.rs` checks only what no single field
    can express — duplicate keys, an empty mapping list, a non-positive version, identifier
    rules. Nothing there re-checks what the types already guarantee, because a check that
    cannot fail is a check that will eventually stop being maintained.
  - **Every file read is bounded, checked before the allocation it governs** (AGENTS.md §4):
    file size from metadata before the bytes are read, file count, mapping count, display-name
    length, identifier length. A directory that cannot be read is an **error, not an empty
    registry**, so a caller that ignores the result cannot mistake a build mistake for a bug in
    its own code.
  - **Errors report every broken rule, not just the first**, so a profile author fixes one file
    in one pass, and each message names its field path. Quoted text is truncated so a malformed
    file cannot produce an unbounded error message.
  - **Three mistakes of my own, caught by the compiler rather than by a reviewer.** A scratch
    helper returned `Result<(), _>` where a value was meant, so every call site inferred `()`
    and could not find `write` on it. Three tests wrote `match load_dir(..)? { RegistryError::X
    => .. }` — the `?` unwraps to a `Registry` **before** the match, so those arms could
    never match, and the fallback arm would have reported a **successful load** as "expected
    Invalid". That last one is the dangerous shape: it fails as a pass-shaped error rather than
    as a compile error.
  - **A seventh scripted edit failed to land**, caught by a guard rather than by reading the
    file: the note anchor `- [ ] **Prerequisites:** T-030` also matches T-031, which has the
    same prerequisite. The guard refused and the note was not written to the wrong section.
  - **First run (496b59d): 18 of 19 passed. The one failure was my own guard refusing to run
    a vacuous test**, which is the guard working rather than the guard failing.
  - **`a_profile_file_larger_than_the_bound_is_refused_before_it_is_read` asserted that its
    own fixture was big enough, and caught it.** The filler was `"x".repeat(2000)` against a
    **64 KiB** bound — a file three percent of the limit, so the loader would have
    correctly *accepted* it and the size bound would never have been exercised. The assertion
    `body.len() > MAX_PROFILE_BYTES` failed first. The filler is now sized **from the
    constant** (`MAX_PROFILE_BYTES + 1024`), so raising or lowering the bound cannot silently
    turn the test into a test of nothing.
  - **This is the second time a self-check has caught my own fixture in this project**, after
    the T-034 case where a `skip` guard made a green job assert nothing. Both were found by
    writing an assertion that could only pass if the test were really testing something, and
    both were worth more than the bug they replaced.
  - **VERIFIED. Run `efba5a7`: all 10 jobs green, read out of the logs.** `Test` **67 passed, 0
    failed**; the database suite **17 passed, 0 failed on each of PostgreSQL 16 and 17**.
    **202 tests across all jobs, 0 failed.**
  - **All five suite/job combinations are an exact name-for-name match** with the source:
    `profiles.rs` 19/19, `policy_unit.rs` 13/13, `invariants_unit.rs` 18/18, and `invariants_db.rs`
    17/17 on **both** PostgreSQL majors. Not just a passing count — each declared test is
    individually confirmed present in the log.
  - **Each of T-032's three Done-when clauses maps to a named test**, so the tick rests on the
    clause rather than on the suite passing: *invalid profile is rejected* →
    `invalid_profiles_are_rejected_with_a_reason`,
    `structural_faults_are_rejected_by_the_types`, the oversize and too-many-mappings tests;
    *default-enabled mappings match each profile* →
    `shipped_profiles_expose_only_their_default_enabled_mappings`, which reads the real files in
    `profiles/` rather than a copy, so editing a profile without updating the table fails here;
    *a sixth sample profile loads with no code change* →
    `a_sixth_profile_loads_with_no_code_change` and
    `a_sixth_profile_alongside_the_five_loads_without_interference`.
  - **What this does and does not establish.** It establishes that the loader loads, validates
    and bounds profile files, and that the five shipped profiles are what `research.md` RQ-1
    says. It says **nothing** about whether those facts are correct — that is what E-1 and the
    L4D1 primary-source search are for, and neither has run. It also says nothing about the
    control plane: `crates/control-plane/src/db/profile.rs` still accepts a `ProfileSpec` as a
    value from its caller, because wiring the registry in belongs to the API layer (T-040/T-042)
    and doing it here would create a `control-plane -> gtt-profiles` edge in a task whose scope
    is the loader.
  - **Two candidates are now open for the owner**: ADR-020 (T-041, authorization error codes)
    and ADR-021 (T-032, evidence gating default exposure). Neither is a blocker for further
    work; both change one match arm and one named test if the preference differs. Before
    pushing, a script transcribed the loader's rules and ran them against the real files in
    `profiles/`: **72 assertions, all passing** — five files parse and validate, the
    default-enabled set per profile is `game` for four and empty for `l4d1`, every profile has
    a non-optional mapping, and no file contains an address or a host:port pair. **That is a
    transcription, not the Rust deserializer, and nothing here is claimed as a passing test.**
- **Scope:** Declarative, versioned profile format; validation at load; default-off optional mappings; custom port support.
- **Deliverables:** `crates/profiles`, `profiles/*.toml|json` for five games.
- **Requirements:** GAME-007, GAME-008
- **Verification:** Unit tests
- **Done when:** Invalid profile is rejected; each game's default-enabled mappings match its profile; a sixth sample profile loads with no code change.

### T-033 — Create migrations from the data model [CP]

- [x] **Prerequisites:** T-003, T-030
- **Scope:** Translate `schema-draft.sql` into `sqlx migrate` migrations (ADR-017); add CI step applying all migrations to an empty DB; retire the draft file per `schema.md`.
- **Deliverables:** `migrations/`; CI step.
- **Requirements:** TEST-007, PRIV-006
- **Verification:** CI migration job
- **Done when:** Migrations apply to an empty database in CI; `schema-draft.sql` removed in the same change.
- **Note 2026-10-17 — verify the tool, do not assume it.** `sqlx-cli` has never been executed in this environment (`dlltool.exe` missing on the GNU toolchain, no Windows SDK on MSVC, no prebuilt binary published), so its behaviour is unconfirmed. **First check: that `sqlx migrate` parses statement boundaries correctly** — the exact defect class that silently produced a zero-table schema during the manual verification recorded in `schema.md`. Then confirm the empty-database result matches the verified baseline: **12 tables, 35 indexes, 57 constraints, 13 foreign keys, 0 columns matching `%email%`**.

- **Progress 2026-10-18 — EVERYTHING BUILT AND LOCALLY VERIFIED; THE BOX STAYS UNCHECKED UNTIL THE CI RUN GOES GREEN.** Deliverables exist: `migrations/20261018120000_initial_schema.{up,down}.sql` and an enabled `migrate` job in `ci.yml` (it was `if: false`; it is not any more). `schema-draft.sql` is deleted in the same change, as the Done-when condition requires, and the traceability audit's model/DDL check now reads `migrations/*.up.sql` so `data-model.md` is still compared against what actually gets applied.
  - **The migration was generated, not transcribed.** `scripts/gen_initial_migration.py` extracts the DDL body from `schema-draft.sql` and writes the `.up.sql`, with a `.down.sql` built from reverse creation order. Hand-copying 200 lines of DDL is exactly the operation that can silently drop a constraint. The generator refuses to overwrite an existing migration and is run once, by hand.
  - **The note above this task asked for a specific first check** — that `sqlx migrate` parses statement boundaries. That could not be answered locally at all: `sqlx-cli` will not build here and `--single` is line-oriented by nature. **It is now answered on a hosted Linux runner**, which has neither limitation, so T-003's environment blocker is closed by circumstance rather than by elevation.
  - **Every CI assertion number was measured, not copied.** Two of my first guesses were badly wrong (`check constraints` 46 vs the true 23; `unique indexes` 12 vs 24). Measuring exposed a trap worth keeping: **`information_schema.table_constraints` is the wrong source for constraint counts.** It reports **102** CHECK constraints where `pg_constraint` reports **23**, and 611 with no schema filter. The `pg_constraint` total, 57, matches the independently measured value already in `schema.md`, so the CI assertions read from `pg_constraint` with an explicit `public`-namespace filter. This view fails silently — it returns a confident wrong number, not an error.
  - **The six negative-test statements were executed against real PostgreSQL 16 before being committed**, which caught two mistakes in my first draft: `tunnels.expires_at` is `NOT NULL` with no default, so a "valid row" insert without it would have failed for the wrong reason; and a second insert reused an `idp_issuer` an earlier step had already taken, so it hit `users_idp_uniq` instead of the constraint under test. Keys are now distinct and the statements are order-independent.
  - **The `postgres --single` line-orientation trap bit a second time.** Splitting the migration on `;` and rejoining with newlines reconstitutes the very defect `schema.md` documents, and produced a zero-table schema again. `scripts/apply_via_single.py` now collapses each statement onto one line and is the only supported local path. The CI job is worth keeping even though the local path now works, precisely because it exercises a parser instead of a line splitter.
  - **All counts are identical on 16.10 and 17.6**, so one assertion set is safe across the matrix.
  - **Counts prove structure, not enforcement**, so the job also runs the four must-reject inserts and two must-accept inserts. Each must-reject one failed with its intended constraint name when verified locally.
  - **A defect that only running this could have found.** `sqlx migrate` creates its own `_sqlx_migrations` table in the target schema. Every count was measured locally via `postgres --single`, which has no such table — so in CI the table count would have been **13, not 12**, and the post-revert count **1, not 0**. Both would have failed for a reason with nothing to do with the schema, and the tempting "fix" would have been to relax the expected numbers instead of finding the cause. They now exclude `_sqlx%` so they describe the schema rather than the tool's bookkeeping.
  - **First CI run (90d315c) FAILED, and what it proved is worth separating from what it did not.** Both matrix legs failed at **Revert and re-apply**, with the message **"revert left tables behind"** — which blames the migration and was wrong. Every step before it passed: `cargo install sqlx-cli --version 0.9.0` resolved, **`sqlx migrate run` applied the schema, and running it twice more was a genuine no-op**. That is the note above this task's first-check answered: **`sqlx migrate` does parse statement boundaries correctly**, which was the one thing this machine could not determine. The cause was that **`psql` does not read `DATABASE_URL`** — that is `sqlx`'s, not libpq's — so every `psql` call had no credentials. Reproduced locally with no runner: with `DATABASE_URL` aimed at `nonexistent.invalid:6543`, psql dialled `localhost:5432`. It was disguised because GitHub's shell is `bash -e`, and a failing command substitution inside `test "$(psql ...)" = "0"` does not trip `-e`, it just evaluates false — so a credential problem was reported as a failed schema assertion, and because the step aborted, the two verification steps after it never ran at all. Fixed three ways: the job sets the `PG*` variables, a preflight step now fails loudly if psql cannot connect, and the revert check no longer buries a connection failure inside a `test`. **`audit_traceability.py` now rejects any workflow that invokes psql without them**, with a planted defect; it was fooled once while being written, because it matched the variable name anywhere in the file and the preflight's own `echo "PGHOST=$PGHOST"` satisfied it, so it now requires an actual assignment. Both branches were exercised by hand.
  - **Not yet verified: that the revert, the structural assertions, and the six negative tests pass end to end.**
  - **Second run (34ffe78) got further and failed differently, which is the useful signal.** `psql can actually connect` passed — the credential fix works — and so did `Apply all migrations`, `Re-running must be a no-op`, and **`Revert and re-apply`, so the `.down.sql` is now confirmed to run through `sqlx migrate revert` on both PostgreSQL 16 and 17.** The failure moved to `Verify the resulting structure`, and the cause was the same `_sqlx_migrations` mistake made once already: **I excluded `_sqlx%` from the table count but not from the index or constraint counts.** sqlx's table declares `version BIGINT PRIMARY KEY`, so it contributes a table, a pkey index, and a primary-key constraint — CI would have seen 36 indexes, 13 primary keys and 58 constraints. **All catalog counts now share one exclusion**, and the constraint queries join `pg_class` so a single exclusion covers every one. Verified locally against real PostgreSQL 16 twice: once with only the schema, and once after creating `_sqlx_migrations` exactly as sqlx does. The counts were identical in both (12 tables / 35 indexes / 13 FK / 23 CHECK / 9 UNIQUE / 12 PK / 57 constraints) while sqlx's objects went from 0 to 2, so the exclusion is what is keeping them right.
  - **`audit_traceability.py` now rejects this class of partial fix**, with a planted defect that reproduces it exactly by dropping the exclusion from the constraint counts only. It resolves `$VAR` references, so a workflow can keep the exclusion in one shared variable, which is the better fix. It was also wrong twice while being written: the first version flagged prose inside YAML comments, a count already scoped to `audit_events`, and the diagnostic that deliberately counts sqlx's objects. A count scoped to one named table cannot be affected by sqlx's table, so those are correctly exempt now.
  - **CI run 10ac6f4 is GREEN: all 8 jobs, including both migration matrix legs.** This is the first time the structural assertions and the six negative tests have executed, and they passed. **The Done-when condition — "Migrations apply to an empty database in CI; `schema-draft.sql` removed in the same change" — is therefore demonstrated by its stated verification, so the box is ticked.** What is verified: `sqlx-cli 0.9.0` installs; the migration applies to an empty PostgreSQL 16 and 17 service container over a normal client connection; re-running is a no-op; `sqlx migrate revert` executes the `.down.sql` and leaves 0 tables; all seven structural counts match the measured baseline; and the four must-reject inserts are rejected by their intended constraints while the two must-accept inserts succeed. **What is still not verified, and is not claimed:** concurrency, transaction isolation under load, and live performance — those need a workload, not a schema, and no run has tested them.
### T-034 — Implement persistence layer and invariant tests [CP]

- [x] **Prerequisites:** T-033
- **Scope:** Data access for all entities; tests for INV-1 to INV-7 including INV-2, INV-4, INV-5 application checks.
- **Deliverables:** Persistence module and tests.
- **Requirements:** SEC-009, TEST-001
- **Verification:** Integration tests on a real database
- **Done when:** Each invariant has a passing test and a failing-input test.

- **Progress 2026-10-18 — BUILT AND LINT-CLEAN, BUT NOT ONE TEST HAS RUN. THE BOX STAYS UNCHECKED.** The Done-when condition is "Each invariant has a passing test and a failing-input test", verified by "Integration tests on a real database". Neither has been demonstrated, so neither is claimed.
  - **What exists.** `crates/control-plane/src/`: `error.rs` (rejection reasons named per invariant, because the caller has to map them to the right `contracts.md` §3 code and guessing is how `forbidden` ends up returned where `not_found` is required); `db/{mod,entities,profile,audit,invariants,accounts,tunnels,relay,sharing,ops}.rs`. Data access for all 12 entities, every owner-scoped function taking the owner id and returning `None` for rows the caller may not see — the foundation T-041 needs, and the only reliable way SEC-009 can be guaranteed rather than reviewed.
  - **The invariant split, which is the design decision worth keeping.** INV-2, INV-4 and INV-5 are **pure functions** and are tested in `crates/control-plane/tests/invariants_unit.rs` with no database. INV-3, INV-6 and INV-7 are **database constraints** with nothing for the application to do, and are tested in `crates/control-plane/tests/invariants_db.rs` against a real PostgreSQL. Writing a second, weaker application-side copy of a rule the database already owns would have been worse than not writing it. Each of the seven has both a passing and a failing-input case, which is what the Done-when asks for.
  - **INV-2 needs a profile registry that does not exist yet.** T-032 is not done and `profiles/` is empty, so persistence accepts a `ProfileSpec` as a value supplied by its caller rather than loading profiles itself. That avoids implementing T-032's loader inside T-034 and avoids a `control-plane -> gtt-profiles` dependency edge before either crate has content. T-040 will pass the real spec in.
  - **INV-5's allow-list is deliberately narrow, and that is a limitation, not a claim.** `contracts.md` does not enumerate the `action` values or metadata keys any route may write, so inventing a complete vocabulary would be an architectural decision taken inside a persistence task. Only five keys are allowed, and adding one is a deliberate reviewed change. What *is* enforced today is the property INV-5 names, on the shape of the data: allow-listed keys, scalars only (never a nested object or array, which is where a payload would arrive), bounded strings, and **no value that parses as an IP address** — the raw-guest-IP rule, and the only part that catches a leak regardless of what the key was called.
  - **Two dependency decisions, both of which were tried and reverted, recorded so they are not undone.** `sqlx`'s **`macros`** feature is off: `query!`/`query_as!` type-check SQL at compile time and so need a live database or a checked-in `.sqlx` cache. **`derive`** is off: it pulls in a proc-macro crate, which must be linked, and **this machine cannot link anything** (no assembler on the GNU side, no Windows SDK on the MSVC side — verified, not assumed). Enabling it would have cost `cargo check`, `clippy` and `test` for the whole crate locally to save ~150 lines of boilerplate, so `FromRow` is generated by a `macro_rules!` macro instead. **`cargo test` consequently no longer runs locally at all**; see the dependency register for the measurement.
  - **Three defects found by running the checks rather than reading the code.** `clippy.toml` had said `msrv = 1.77` while `Cargo.toml` said 1.94, so clippy had been linting against the wrong MSRV since ADR-008 — clippy only started reporting the disagreement once a crate actually needed the newer one. Clippy's `too_many_arguments` fired on four functions taking eight positional arguments, and it was right: `game_profile_id` and `profile_version` are one fact split across two parameters where swapping them compiles. Each is now a named input struct — no `#[allow]`. And the audit's own new sqlx-exclusion check was caught by its self-test resolving `$CAT` file-wide, so with two jobs defining it, the last definition won and a planted defect was checked against the wrong job's correct code. Resolution is now per step.
  - **`cargo deny` passes on the real 167-crate graph** — advisories ok, bans ok, licences ok, 7 duplicate-version warnings, exit 0.
  - **First CI run (5d31210): the `db-tests` job passed on BOTH PostgreSQL 16 and 17.** Every step green — migrations applied, the resulting counts matched the measured baseline, the tests ran, and the `--list` check confirmed the suite was not empty. **So INV-2, INV-3, INV-4, INV-5, INV-6 and INV-7 all have a passing test and a failing-input test that have now actually executed against a real database**, and INV-1 is covered by both a schema scan and a field-name scan in the local suite.
  - **The second run (6033014) took the `test` job red again, and the first diagnosis was wrong.** The skip guard was indeed wrong and is fixed (keyed on `REQUIRE_DB_TESTS` now, not `CI`), but that was **not** what was failing: the `test` job has no database, so the guard's `CI` branch never applied there anyway. The real cause was in the one suite only that job runs — `invariants_unit.rs`, which had never executed anywhere. It asserted that `203.0.113.5/32` parses as an `IpAddr`. It does not: **Rust's `IpAddr::from_str` rejects CIDR notation.** The assertion was also testing the wrong thing — re-asserting a stdlib's parser rather than this module's contract.
  - **The failure was pointing at something real.** `guest_admissions.source_ip` is a PostgreSQL `inet`, which *does* accept CIDR, so `assert_valid_source_ip` is deliberately stricter than the column it guards: an admission grants relay access to **one guest address**, and admitting a prefix would widen one guest's grant into a subnet. That is now the documented contract, the test exercises the real function rather than `IpAddr` directly, and CIDR rejection is asserted with its reason. **Recorded because the temptation was to relax the assertion to match the wrong belief**, which would have turned a correct security property into a test that passes because it tests nothing.
  - **Third run (87be4cb): the test still failed, and the second diagnosis was also incomplete.** The CIDR fix was correct and necessary — `203.0.113.5/32` really is rejected — but it was not the only wrong expectation. The log names the failing assertion at line 454, and the case is `" 203.0.113.5 "`: `assert_valid_source_ip` **trims before parsing**, so whitespace-padded addresses are *accepted*, and the test demanded they be refused. Two CI cycles were spent guessing before the log was read.
  - **Both wrong expectations were the test's fault, not the function's, and both were wrong in the same direction** — asserting that something the function deliberately accepts should be refused. Whitespace tolerance is right for a value arriving over the wire. Fixing the test rather than the function was the decision, and it was verified before pushing this time: all **19** cases were run through a simulation of the exact `s.trim().parse::<IpAddr>().is_ok()` expression, which is how the third cycle was avoided.
  - **The simulation also surfaced a second divergence worth pinning down.** Rust's `IpAddr::from_str` has no notion of an IPv6 zone id, so `fe80::1%eth0` is refused even though `inet` accepts it. That turns out to be the right answer: the live-admission unique index is on `(tunnel_id, source_ip)`, and `fe80::1%eth0` and `fe80::1%eth1` would be two rows for one host, fragmenting the admission set. Both divergences are now documented on the function with their reasons, because "stricter than the column" is only a virtue when you can say where the line is.
  - **CI logs are now fetchable, and that is worth recording because it was the blocker.** The Actions logs endpoint returns **403 unauthenticated and 401 with the push credential**, but `GET /actions/runs/{id}/logs` — the run-level **zip** of every job log — works with it. No `gh` CLI and no API token exist in this environment, which is why two diagnoses were guesses. No script reading the credential is committed: the technique belongs to the maintainer, not to the repository.
  - **Diagnosed by simulating the test bodies in Python**, since `cargo test` cannot run here. That also confirmed the two INV-1 scans find nothing in either the migration or `entities.rs`, so those two tests were never the problem.
  - **The first diagnosis was wrong and that is worth stating plainly.** Fixing the guard was still correct — it was a real defect that would have made seven invariant tests no-ops in the database-less job — but it was not the red. The second run is what proved it, since the job stayed red.
 The guard that stops the database tests silently skipping was keyed on `CI`, so it also fired in the plain `test` job — which runs in CI and has no database service by design. `CI` says where the code is running; it does not say whether this job is supposed to have a database, and only the job knows that. It is now keyed on `REQUIRE_DB_TESTS=true`, set on `db-tests` alone. **The alternative was to delete the guard, which is precisely the "make the gate green by removing the gate" move this project keeps rejecting**, so the guard was corrected instead.
  - **Run 2e387d1 was all-green across all 10 jobs — AND THAT WAS THE PROBLEM.** Reading the log instead of trusting the badge: the `db-tests` job printed `REQUIRE_DB_TESTS=true: the database tests will run` and then **every database test printed SKIPPED**, reporting `11 passed`. **A green job had tested nothing.**
  - **The cause: `SET` does not accept a bind parameter.** `Database::from_pool` ran `SET statement_timeout = $1`, which PostgreSQL rejects as a syntax error (verified locally against 16.10), so **`Database::connect` always failed**. `pool()` turned that failure into a skip with a note on stderr, and the skip guard only checked that `DATABASE_URL` was *set*, never that the connection *worked*. Three layers, each individually defensible, and together they turned a total failure into a green tick.
  - **Three fixes, and the important one is structural rather than another check.** `from_pool` now uses a literal — it has to, because `SET` takes no bind parameter **and** sqlx itself rejects a non-literal query string, so the dependency caught the interpolation I first reached for. `pool()` now treats a connection failure as fatal whenever a database was promised. And `require_db!` asserts rather than skipping, so **a test cannot report success while having done nothing**; the CI step no longer has to scrape output to notice. A separate CI step remains as an independent second net, and both were **verified against the real 2e387d1 log**: the SKIPPED grep finds its 10 lines, the `passed`/`failed` extraction yields 11/0 on a healthy run and nothing on a failing one.
  - **The `--list` check I added earlier was too weak and I have said so in the file.** It proved the suite was not *empty*, which is not the same as proving the tests *ran*, and the difference is exactly what let this through. It has been replaced.
  - **Two constants now have to be written twice** — a millisecond value and a SQL literal — because one cannot be interpolated into the other. That is a drift risk a comment does not close, so `statement_timeout_literal_matches_the_constant` asserts they agree. It deliberately has **no** `assert!(MS > 0)`: that is a constant expression and `assertions_on_constants` rejected it, correctly, since it always passes and so tests nothing.
  - **Run 661eb0a: the database tests ran for the first time — 7 passed, 4 failed, and all four failed identically with `"fixture relay was not created"`.** The bug was mine and obvious once named: `list_allocatable_relays` filters on `status = 'healthy'`, a new relay node defaults to `provisioning`, and each test listed healthy relays **before promoting its own**. The usage test never promoted one at all, which is why it failed too. The error message was wrong too — the relay had been created, it just was not yet allocatable — which is the kind of message that sends you looking in the wrong place.
  - **`make_relay` now returns the node it created and promotes it.** That also removes a latent flake I had not noticed: every test in the file shares one database and they run in parallel, so "the first healthy relay" was whichever test got there first, not necessarily this one's. **No fixture in this file may be found by listing any more.**
  - **INV-4's test was also weak and is now real.** It published a tunnel's *own* mapping and asserted that worked, then checked the other tunnel's mappings existed — which never exercised the cross-tunnel case at all. It now publishes tunnel B's mapping on tunnel A's allocation, asserts refusal, **and asserts no port row was left behind**. INV-6 gained a matching case for a different protocol on the same numeric port.
  - **Three tests added while the file was being fixed:** that a sweep is idempotent and satisfies INV-7, that `list_tunnels` is bounded whatever limit a caller passes (`AGENTS.md` §4 forbids unbounded allocation on caller-controlled input), and that a suspended user's status is available for the entitlement decision T-041 will make — with a comment saying plainly that `start_tunnel` is **not** that check.
  - **The file was rewritten rather than patched.** Four rounds of scripted edits had left it with a brace mismatch that only the compiler found, because each patch matched text that rustfmt had already reformatted. Rewriting it in one pass with every fix incorporated was faster and safer than a fifth patch, and the lesson is recorded: **when a scripted edit has failed to match three times, stop patching and rewrite.**
  - **14 database tests and 17 unit tests now exist**, up from 11 and 16.
  - **Run 0bb6c96: 11 passed, 3 failed — and all three were real, which is the point.**
  - **A genuine library bug: `usage_since` could never have worked.** PostgreSQL widens `sum()` over `bigint` to **`numeric`**, because a sum of bigints need not fit in a bigint, so decoding straight into an `i64` fails with `ColumnDecode`. Decoding `numeric` in Rust would mean adding `rust_decimal` or `bigdecimal` for one aggregate, which §5.2 rules out, so the cast is in SQL — with a `least(...)` bound, because without it a relay reporting enormous figures could walk the total past `i64` and turn a quota check into a decode failure. Clamping reports the ceiling, which is what a quota check wants anyway.
  - **A case where the database was right and the test was wrong.** INV-6 gained an assertion that the same numeric port on a different protocol does not collide — and it failed, because `allocation_ports` has a composite foreign key `(mapping_id, protocol)` to `tunnel_mappings`. The fixture's mapping was UDP, so publishing it as TCP violated the FK. The schema was doing precisely its job; the test had to ask for something real, so the fixture now carries both a UDP and a TCP mapping and mappings are selected **by protocol rather than by position**.
  - **The schema refused to let the sweep test be arranged the obvious way.** `tunnels_expiry_chk` is `expires_at > created_at`, and `created_at` defaults to the *transaction* start time — later than the `Utc::now()` Rust computed a moment earlier — so the one-second margin the test used was a race, and it lost. Making a tunnel "due" by pushing `expires_at` into the past violates the same CHECK. The fixture is now an explicit INSERT with both timestamps in the past, and the comment says why rather than leaving a future reader to rediscover it.
  - **INV-6 also gained a release-and-reuse case**, so the index is shown to be on unreleased rows rather than being a permanent reservation.
  - **Run 431d159 is green on all 10 jobs, with 14/14 database tests passing on PostgreSQL
  16 AND 17 and zero skips — confirmed from the logs, not from the badge.** The box is
  ticked. Coverage mapped per invariant, because "the suite is green" is not the same claim
  as "each invariant has both cases":

  | Invariant | Passing | Failing-input |
  |---|---|---|
  | INV-1 no local target | schema scan, entity-field scan | `rejects_a_local_target_field`, and the scanner tested against a planted target |
  | INV-2 profile match | `accepts_a_mapping_the_profile_declares` | wrong protocol, undeclared key, wrong profile version, and a DB-level rollback case |
  | INV-3 one live allocation | first reservation succeeds | second refused, and re-allowed after release |
  | INV-4 mapping in allocation's tunnel | `accepts_a_mapping_from_the_allocation_tunnel` | `rejects_a_mapping_from_another_tunnel`, and the DB case asserting no port row is left behind |
  | INV-5 audit metadata | `accepts_allowed_metadata` | raw IP, non-allow-listed keys, nested objects, arrays, oversized values |
  | INV-6 one live mapping per port | first publish, and TCP/UDP sharing a number | second UDP publish on the same port refused; released port reusable |
  | INV-7 terminal has `stopped_at` | `stop_tunnel` writes both | the raw UPDATE setting `stopped_at = null` is rejected, and the sweep is shown to satisfy INV-7 too |

  **What this task found that no amount of reading would have.** Six CI rounds. Three
  defects in my own guards, **one of which made a green job assert nothing at all**, and
  three genuine bugs in code that looked correct: a library function (`usage_since`) that
  could never have worked, and two places where the database was right and the test was
  wrong. The pattern is the same one this project has kept hitting — writing is cheap,
  running is where the truth is.
### T-035 — Implement server-manager crate [CP]

- [ ] **Prerequisites:** T-030
- **Progress 2026-10-18 — THE VERIFIABLE CORE IS BUILT AND LINT-CLEAN. THE BOX STAYS
    UNCHECKED, because the headline requirement cannot honestly be claimed yet.**
  - **Why it is not ticked.** T-035's Done-when opens "A tampered artifact is rejected before
    execution". There is currently **no trustworthy digest for the Minecraft Java `server.jar`**,
    and `research.md` RQ-13 says so in `[X]` terms: *"Not confirmed: that the manifest was
    actually fetched and that a manifest-sourced SHA-1 verifies for the `server.jar` object
    specifically (the manifest hashes the version JSON, not necessarily the server jar). **E-15
    must verify the real check end to end before `SRV-002` can be claimed.**"* A rejection
    control built on a digest whose source is unresolved is a control that reads as authoritative
    and is not, which is the exact failure `GAME-006` and `SRV-003` discipline exists to prevent.
  - **What is built**, being the parts that need no unresolved fact:
    - `verify.rs` — SHA-256 comparison, streamed in a 64 KiB buffer with a **512 MiB cap**
      checked from metadata before the bytes are read (`AGENTS.md` §4). The error names the
      file so the host knows which download failed and **never echoes its contents**
      (`PRIV-002`/`SEC-013`). Digest comparison is constant-time; the derived `PartialEq` is not,
      and `Digest::matches` exists because of that.
    - `config.rs` — `server.properties` generation where `SRV-004` is **structural**: a
      request cannot represent RCON or query as on without a secret, and the flags are written
      **unconditionally** so regenerating over a hand-edited file turns them back off. Ports come
      from the profile's default-enabled mapping, never from the caller.
    - `paths.rs` — per-user data directory. There is **one** way to turn a name into a path
      and it cannot leave the root: `..` is normalised away, absolute components are refused
      rather than joined (`Path::join` would discard the root), and the **nearest existing
      ancestor** is canonicalised so a symlink inside the root pointing out is caught.
    - `provision.rs` — the `SRV-003` gate. It returns **nothing**, and that is the feature
      working: no acquisition experiment has passed. The refusal message names E-15 and E-16 so a
      host can act on it.
  - **Acquisition is deliberately not implemented.** No HTTP client, and no recorded source URL
    for the jar. `ArtifactSource` is the seam a future caller fills in, which is the same shape as
    T-041's `ProfileSpec` seam. The Adoptium JRE path **is** `[V]` with SHA-256 verified and 12/12
    checks, so the verifier has a real caller today.
  - **A real bug, found by simulation and not by the Rust tests.** `resolve` originally
    canonicalised only the *immediate* parent, so `servers/minecraft-java/world` was refused
    whenever neither `minecraft-java` nor `world` existed — **precisely the case provisioning
    creates.** Every Rust test in the file used a path one level deep, so none would have caught
    it. A transcription of the normalisation logic run against a real directory tree found it in
    one go. Fixed by walking up to the nearest existing ancestor, and there is now a named test
    for the multi-level case.
  - **Symlink confinement is implemented but UNTESTED in CI.** Creating a symlink on Windows
    needs elevation (`WinError 1314`), which `SRV-010` forbids and this environment cannot do. The
    simulation skipped that case too. **The canonicalising ancestor walk is what makes the check
    work, and that specific defence has no test.**
  - **One new crate, measured not assumed.** `sha2 0.11` for SHA-256; `const-oid 0.10.2` follows
    it in, so the graph goes **167 → 168**. `sha2 0.11.1` was already present via
    `sqlx-postgres`, but taking it as a *direct* dependency activates a feature path that pulls in
    `const-oid`. The natural assumption — "sha2 was already there, so nothing is added" — is
    wrong, and the register says 168.
  - **The vacuous constant test was deleted, not worked around.** A test asserting
    `MIN_SECRET_LEN == 16` cannot fail unless someone edits the test and the constant together.
    Replaced with a boundary test: a secret of exactly `MIN_SECRET_LEN` characters is accepted
    and one character shorter is refused, so **lowering** the floor fails one case and **raising**
    it fails the other.
  - **First run (`edfb4da`): 24 of 31 tests passed and the SEC-017 dependency scan FAILED.**
    Both failures were real, and one of them was a security gate, so neither was dismissed.
  - **The dependency scan failed on `wildcards = "deny"`, and not where I assumed.** I read the
    word "wildcard", saw `sha2 = "0.11"` in my own manifest, and assumed that was it. It was
    not: a path dependency with **no `version` field** is a wildcard requirement to cargo-deny,
    so `gtt-profiles = { path = "../profiles" }` was the offender. Fixed by adding
    `version = "0.0.0"` to match the workspace version. **A security gate I nearly dismissed by
    pattern-matching rather than reading is the worst version of this mistake**, and the log
    said exactly which manifest line and which column it was.
  - **All six path tests failed with `NotCanonicalisable { supplied: "app" }`, and the tests
    were right to fail.** `DataPaths::new` required the caller to have created
    `%LOCALAPPDATA%\<app>` already. Six tests failing for "you forgot to do the obvious thing
    first" is a sign **the API is wrong, not the tests**, so the constructor now creates the
    directory. A caller that must remember `create_dir_all` before `new` can get the containment
    check wrong by omission, which is the one failure this module exists to prevent.
  - **A malformed-digest case was not malformed.** The test asserted that
    `"a".repeat(63)` plus one more character should be refused. That is exactly **64** hex
    characters, which is a valid SHA-256 digest, so `parse_hex` accepted it correctly. The case
    now uses 65. **The runner was right and the test was wrong** — the same shape as the
    T-032 oversize fixture, and the third time a self-written fixture has been the defect.
  - **Verified locally after the fixes**, before pushing: `cargo deny check advisories bans
    licenses` exits **0** with `advisories ok, bans ok, licenses ok`, and all five simulations
    still pass (config **26**, paths **19**, profiles **72**, policy **244**, and no denied
    lint construct in real code).
  - **VERIFIED FOR WHAT IT COVERS. Run `9046efc`: all 10 jobs green**, including the SEC-017
    dependency scan that the previous run failed. `Test` **98 passed, 0 failed**; the database
    suite **17 passed, 0 failed on each of PostgreSQL 16 and 17**. **264 tests across all jobs,
    0 failed**, and all **six** suite/job combinations are an exact name-for-name match with the
    source — `server_manager.rs` **31/31**, `profiles.rs` 19/19, `policy_unit.rs` 13/13,
    `invariants_unit.rs` 18/18, `invariants_db.rs` 17/17 on both PostgreSQL majors.
  - **The box stays unchecked anyway, and that is the honest reading of the rule.** T-035's
    Done-when opens "A tampered artifact is rejected before execution". The verification
    machinery is proven — a tampered artifact, an extended one and a truncated one are all
    refused, and a missing file and a directory are refused rather than treated as nothing-to-do.
    What is **not** proven is that any real game's digest can be trusted, because E-15 has not
    verified it end to end. Ticking because "the code I wrote is tested" would substitute my own
    condition for the task's, which AGENTS.md §11 forbids.
  - **What remains for T-035, in order:**
    1. **E-15 end to end** on a clean standard-user VM — fetch the manifest, verify a real
       `server.jar` against a digest obtained from an official source, and record the source URL
       and verification method per game, which is `SRV-002`'s own stated verification. Blocked on
       `SRV-001`, which needs a retail standard-user Windows image.
    2. **Acquisition**, once there is a recorded channel to call. `ArtifactSource` is the seam.
    3. **A symlink test.** The canonicalising ancestor walk is what makes `SRV-010`'s containment
       hold against a link pointing out of the root, and creating a symlink on Windows needs
       elevation — which `SRV-010` forbids and which this environment cannot do. **That
       specific defence is implemented and untested**, and it is the one thing here that would
       most repay a real machine.
    4. **Process supervision** (`SRV-002`'s "before execution" half, `SRV-008` world survival,
       `SRV-007`), which belongs to T-025/T-026.
  - **A correction to my own record-keeping, in case it is read later.** An earlier draft of this
    note claimed E-15/E-16 had produced **no recorded findings at all**, on the strength of a
    grep for `**E-15**` that returned nothing. That was wrong, twice over: the tags are written
    as `[E-15]` and `SRV-018`, and the findings are extensive — real Mojang piston URLs,
    the Adoptium API, the SHA-256-verified JRE, 12/12 checks. **A grep that finds nothing is
    evidence about the grep**, and this is now the third time one has misled me in this project.
  - **Not yet verified: nothing further is claimed here.** The corrected run above is the whole of
    it.
    transcriptions were run: the configuration generator (**26 assertions**, including every
    `SRV-004` refusal and the secret-length boundary in both directions), and path confinement
    against a real directory tree (**19 assertions**, including traversal, absolute components and
    the inner-`..`-is-allowed complement). **Those are transcriptions, not the Rust, and nothing
    here is claimed as a passing test.**
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
- **Added 2026-10-17 (ADR-018):** the challenge method is fixed to **`S256`** and a `plain` challenge must be rejected with `validation_failed`. "Done when" gains: a `plain` challenge is rejected. RFC 9700 §2.1.1: *"Currently, S256 is the only such method."* [V]

### T-041 — Implement authorization policy and tenant isolation [CP]

- [x] **Prerequisites:** T-034
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

- **Progress 2026-10-18 — BUILT, LINT-CLEAN, NOT YET RUN. THE BOX STAYS UNCHECKED.** `crates/control-plane/src/policy.rs` implements the matrix; `crates/control-plane/tests/policy_unit.rs` has **13 tests** and the new `sec009_*` tests in `crates/control-plane/tests/invariants_db.rs` bring that file to **19**. Nothing has executed yet, so nothing is claimed.
  - **The matrix is mostly underspecified, and that shaped the whole design.** It names an error code for **2** of its cells and leaves **19** as a bare "no". A hand-written expected-answer table would only have restated the implementation, so the central test instead asserts the one thing the document actually states as a rule: **a caller who cannot see a resource gets the same answer whether it belongs to someone else or does not exist.** That is asserted over every actor and every operation by comparing the two decisions. It catches the class of bug where a policy answers `forbidden` for a resource it found and `not_found` for one it did not — the exact oracle the contract forbids.
  - **The rule divides the cells in two, and each half is only safe because it is uniform.** Capability failures (a relay creating a tunnel, a user reporting health, a non-admin suspending) answer `forbidden` for *every* target. Visibility failures (cross-owner access, another relay's state) answer `not_found` for *every* resource not theirs. Uniformity is what makes each safe: a response that varied with existence would confirm it.
  - **ADR-020 raised as a CANDIDATE, because the choice is the owner's.** Recorded in `research.md` §6a with the assumption spelled out. **19 cells had no code in the contract and the implementation had to pick one.** The one cell that is a reading rather than a deduction is "terminate tunnel, called by the owner": implemented as a capability failure (`forbidden`, uniform) because the matrix grants that row to nobody but the admin, and granting it to nobody is a statement about the caller rather than the target. The alternative reading (`not_found` for a tunnel you do not own) is also safe, so the property does not settle it — **if the owner prefers it, it is one match arm and one named test.** Three smaller assumptions are listed alongside it: a suspended owner may still view, stop, revoke and list; a guest is answered `unauthenticated` rather than `forbidden` because under ADR-007 G2 it never reaches this layer; and admins see all users' resources.
  - **Two bugs found while writing the policy itself, before any test ran.** `suspended_user_may` existed but nothing called it, so a suspended account would have been able to create and start tunnels — exactly what `data-model.md` forbids. And the capability arm matched on the operation alone, so `(_, CreateTunnel) => Allow` would have let a **relay** create a tunnel. Dispatch is now by actor kind first, which puts each matrix column where it belongs.
  - **A third bug found by writing the test that the code could not catch.** The admin was exempted from visibility entirely, and because `Target::UserOwned { owner: None }` is also what a **nonexistent** resource resolves to, an admin asking about a tunnel that did not exist would have been told **allowed**. An admin is entitled to learn that a resource exists; not entitled to be told a nonexistent one is fine. There is a named regression test for it.
  - **The ordering is load-bearing and documented as such: visibility before entitlement.** A suspended user asking about *someone else's* tunnel gets `not_found`, not `forbidden`, so account state cannot be used as a side channel either.
  - **First run (6151449): all 18 database tests passed on PostgreSQL 16 and 17, including the four new `sec009_*` security tests. The policy tests failed 3 of 13, and all three were informative.**
  - **A real design hole, found while fixing the first failure.** The policy refused every anonymous request, and under ADR-007 G2 **the claimant IS anonymous** — the guest has no account and no client, and the invitation secret is the authority. The claim route was therefore unreachable as written. The claim row is now answered before the credential check, requires `Target::None` so it cannot be used to probe a resource, and is granted to every actor except a relay and an admin, which the matrix marks "no". The restructuring removed the early return entirely: every actor's answer is now in one exhaustive dispatch, so adding an actor variant fails to compile rather than falling through. The compiler enforced that — it rejected the unreachable arms an early return would have left behind.
  - **The matrix has no column for the anonymous claimant, which is a gap in the document and not in the implementation.** It has Owner / Other user / Guest / Relay / Admin and marks the claim row "n/a" for both Owner and Guest, so no column covers a guest with no account. Recorded in `research.md` §6a with a recommendation that `contracts.md` either gain an anonymous column or re-word the row — not changed here, because the API surface belongs to T-040 and re-wording a contract is not this task's to do.
  - **A test proved a claim in my own documentation wrong.** `research.md` §6a said terminate-by-owner answers `forbidden` for *every* target. It does not: `forbidden` for your own tunnel, `not_found` for someone else's and for a missing one — and that difference is safe, because the owner can already tell "mine" from "not mine", and the pair that must match (*someone else's* against *missing*) is `not_found` on both. Both the test and the ADR text were corrected. **The property test had been asserting something stronger than the contract requires, and the code was right.**
  - **A third failure was a test bug that would have been easy to misread as a policy bug.** The indistinguishability test fed `CreateTunnel` a `UserOwned` target, which never occurs — nothing resolves a tunnel id when creating one — and reported a mismatch. The property is only meaningful over a pairing that can happen, so each operation now declares the target kind it actually uses.
  - **Second run (709877f): the same three tests failed, for two different reasons — one
    of them a regression I had just introduced.**
  - **A leak I created while fixing a leak.** Removing the early
    `Anonymous => Unauthenticated` return, in order to fold everything into a single dispatch,
    let anonymous callers fall through to the **visibility** check. They then got `not_found`
    for one target and `unauthenticated` for another — **precisely the oracle this module
    exists to prevent**, now aimed at anonymous callers instead of cross-owner ones. The
    credential check went back ahead of visibility, with the claim row exempted so the route
    stays reachable. The compiler enforced the restructuring as a side benefit: it rejected the
    unreachable arms an early return leaves behind, which is what forced the single exhaustive
    dispatch in the first place.
  - **A test bug that would have read as a policy bug.** The indistinguishability test used the
    *owner's* own id as "someone else's", so for the owner actor the comparison target was its
    own and the two answers legitimately differed. The property needs an id belonging to **no**
    actor.
  - **Third run (e669401): 11 of 13 passed. Both remaining failures were test bugs, and the
    code was right in both.**
  - **A sanity assertion that asserted nothing and then failed.** The check that the
    comparison ids belong to no actor looped over a list that *included* those ids and asserted
    they differed from themselves, so it failed on its own first element. Rewritten to compare
    the four **actor** ids against the two comparison ids, which is what the loop relies on.
  - **A carve-out that never reached the test it belonged to.** The script carrying it raised
    on an earlier unmatched anchor before writing. Found by grepping for the carve-out after the
    run and finding one occurrence where there should have been two. Both of this run's bugs
    are the same shape as three earlier ones: a scripted edit reports nothing, and the next step
    trusts it.
  - **Built a stand-in for the runner, because three rounds running the only thing that found
    these was the runner.** A transcription of `decide()` into Python walks the same actor list,
    operation list, target pairing and assertions as `policy_unit.rs`, so a self-defeating
    assertion or a mispaired target is caught locally instead of costing a CI cycle.
    **244 assertions, all passing**, plus a closing check that `policy.rs` still contains each
    rule the transcription depends on, so the transcription cannot silently drift.
  - **What that simulation does and does not establish.** It verifies the **tests** are
    self-consistent and that the reasoning about the policy holds across every actor and
    operation. It does **not** verify the Rust behaves as transcribed, and the transcription
    was wrong three times in ways that made it disagree with *correct* code: it treated an
    admin as a user with id `None`, omitted the two operator-only rows from the admin's allowed
    set, and referenced an undefined `aid` that silently picked up a leaked loop variable. It
    is a cheaper gate, not a substitute for the runner, and **nothing here is claimed as a
    passing test.**
  - **A lesson worth more than the fix.** A `git commit -m` whose message contained an
    apostrophe caused a PowerShell parse error, and PowerShell parses a block before running
    any of it — so a note script sitting earlier in that same block never ran at all. The
    later commit succeeded from a message file, so the gap was invisible in git history: the
    commit existed, the note did not, and the next script anchored on text that was never
    written. **Commit messages go in a file, and no script's success is trusted without a grep
    that shows its text inside the section it was meant for.**
  - **VERIFIED. Run `fd581ab`: all 10 jobs green, and read out of the logs rather than the
    badge.** `Test` **48 passed, 0 failed**; `Invariant tests against a real database` **18
    passed, 0 failed on each of PostgreSQL 16 and 17**. **168 tests total, 0 failed.**
  - **Counts are not enough, so the test *names* were cross-checked against the log.** The
    database suite is an exact **18-for-18 name match** with the source, and the policy suite
    an exact **13-for-13**. All four `sec009_*` security tests are individually confirmed
    present in the log. This is written down because a green job that runs nothing has already
    happened once in this project, and a count alone would not have caught it.
  - **What the verification does and does not cover.** It covers `decide()` as a pure
    function, and the data layer's owner-scoping against two real users in a real database.
    It does **not** cover the HTTP surface, token handling, or admin authentication — no
    API layer exists yet, so nothing here says anything about `contracts.md`'s routes being
    correct. **SEC-009's "Done when" — cross-user reads and writes all return `not_found`
    — is demonstrated at the policy and persistence layers, which is all this task builds.**
    The API layer that must actually emit that code is T-040's, and until it exists the
    guarantee is a property of two components rather than of the service.
  - **The tick is on the Prerequisites line**, which is this file's convention; confirmed
    against the already-ticked T-034 rather than assumed.
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
- **Scope:** Admin identity model **decided** — ADR-019: the project owner is the sole administrator, admin is a separate identity rather than a role on `users`, WebAuthn required with no TOTP fallback, short sessions plus step-up before destructive actions, offline recovery codes. Still to do here: spike whether WebAuthn can actually be enforced on the admin client (unverified); implement admin routes: suspend, terminate, relay drain.
- **Deliverables:** ADR-019 (done); admin identity store and its migration; admin routes; tests.
- **Requirements:** SEC-018, SEC-015
- **Verification:** Design review; security tests
- **Done when:** Admin routes fail without admin authentication; all admin actions audited. **Added 2026-10-17:** a user access token presented to an admin route returns `forbidden`; a destructive route refuses a session that has not step-re-authenticated; recovery codes restore admin access and are hashed at rest; no recovery material appears in logs or `audit_events` (INV-5).


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
- **Progress 2026-10-17 — substantially done ahead of the task, and the box stays unchecked.** `scripts/audit_traceability.py` already covers requirement-to-task mapping, task-to-requirement links, prerequisite cycles, cited-path existence, longest-chain depth, ADR table/record agreement, **`data-model.md` against `schema-draft.sql` column by column for all 12 tables**, and — added 2026-10-17 — **`api.md` against `contracts.md` for all 20 routes**, checking method, path, and success status in both directions. Thirteen planted defects prove the checks fail when they should; five of those are for the model/DDL and route checks. The route check found a real defect on first run: `contracts.md` §5.12 and §5.13 stated no success code while `api.md` asserted one, and both sections were corrected. **Still open here:** diagram agreement, and any consistency question that needs semantic judgement rather than name matching. Those are why this is not checked.

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
