# Game Server Tunneling Tool (working title)

**Status: planning phase.** There is no application code yet. This repository currently contains the architecture and planning documents only. Nothing described below as "will" exists, and no game has been confirmed compatible.

> **Placement note (resolved 2026-10-17, T-001):** this file, `AGENTS.md`, and
> `CLAUDE.md` are at the repository root. All relative links are correct as
> written. Note that the rest of the documentation set also lives at the root
> rather than under `docs/`; `architecture.md` §4 describes `docs/` as holding
> the documentation set. That inconsistency is **recorded, not resolved** — see
> `architecture.md` §4 and §21.

## What it is

A Windows desktop app that lets a person host a multiplayer game server on their own PC and let friends join over the internet without router port forwarding, a public IP address, or NAT configuration. The host starts a tunnel and shares an invitation; the app connects out to a relay, which forwards friends' game traffic to the local server.

## Planned features and initial game support

| Planned feature | Notes |
|---|---|
| Tunnel for TCP and UDP game traffic | Relay-first; direct peer-to-peer is deferred |
| Expiring, revocable invitations | Guest admission design depends on experiment E-13 |
| Honest status | Separate indicators for tunnel, local server, and observed guest traffic |
| Windows per-user install, signed updates | No administrator rights in normal use |
| **Host-local game server setup** | **Added 2026-10-17.** The app can download, verify, configure, and supervise a game server on the host's own PC. First cut is **Minecraft Java Edition only**, and only if experiment E-15 confirms the acquisition path. Bedrock was deferred: no artifact integrity mechanism could be established, so it fails the verification requirement. **Not verified; no game is promised yet.** The app does not host game servers on operator infrastructure (`context.md` NG-1) |

| Game | Planned scope | Verified compatible? |
|---|---|---|
| Terraria | Direct connect by address | **No** (untested) |
| Minecraft Java Edition | Direct connect by address | **No** (untested) |
| Minecraft Bedrock Edition | Direct connect by address | **No** (untested) |
| Left 4 Dead | Direct connect by address | **No** (untested) |
| Left 4 Dead 2 | Direct connect by address; lobby join only if experiments pass | **No** (untested) |

A game will be called supported only after a real-client join test passes (see `research.md`, ADR-011).

## Architecture summary

Four parts: a **desktop app** (UI plus a Rust tunnel engine), a **control plane** (accounts, tunnel lifecycle, authorization, relay allocation), a **relay** (terminates the host's QUIC connection and owns public ports for each tunnel), and **game profiles** (declarative per-game data). The control plane never stores the host's local server address, and the relay never connects to addresses chosen by clients. In the default design the relay can read game traffic; transport encryption is not end-to-end. Details: `architecture.md`.

## Prerequisites and development environment

Not yet defined. The documents propose, subject to experiment results and decision gate G1: Windows 10/11 for the client, a Rust toolchain, PostgreSQL for the control plane, and the WebView2 runtime for the UI. Exact versions are not decided. Do not treat this list as a verified setup.

## Installation and setup

Not available. There is nothing to install.

## Environment variables

**None are implemented.** The names below are *illustrative placeholders* to show the intended kind of configuration; final names are set during implementation and this section will then be replaced with verified documentation.

| Illustrative name | Purpose | Example (placeholder) |
|---|---|---|
| `GTT_DATABASE_URL` | Control-plane database connection | `postgres://USER:PASSWORD@localhost:5432/DBNAME` |
| `GTT_RELAY_PUBLIC_HOST` | Public host name clients are told to use | `relay.example.invalid` |
| `GTT_SIGNING_KEY_PATH` | Location of the token signing key (never committed) | `/path/to/key` |
| `GTT_LOG_LEVEL` | Log verbosity | `info` |

Secrets must never be committed or logged (`AGENTS.md` §7).

## Local development

**Not yet valid.** Becomes valid after tasks T-001 (repository skeleton) and T-030 (project skeleton and dependency policy) are verified. Planned shape: start a local PostgreSQL, apply migrations, run the control plane, a relay, and the desktop app, plus a local game-server container for compatibility testing. Exact commands will be added only after they have been run.

## Tests and quality checks

**Partially verified 2026-10-17** against `rustc 1.99.1` (installed via rustup). These were **run**, not assumed:

| Command | Result |
|---|---|
| `cargo check --workspace` | Pass |
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | Pass, zero warnings |
| `cargo test --workspace --all-targets` | Pass under `stable-x86_64-pc-windows-gnu` — **0 tests**, the crates are empty skeletons |
| `cargo deny check advisories bans licenses` | Pass, exit 0 — via the official prebuilt `cargo-deny` 0.20.2 binary, SHA-256 verified |
| `python scripts/audit_traceability.py` | Pass — 110 requirements, 110 traceability rows, 86 tasks, 19 experiments, 19 ADR rows (6 with full records), 7 invariants, 12 model/DDL tables compared, longest chain 23, **0 errors, 23 warnings** |
| `python scripts/audit_traceability.py --self-test` | Pass — all **10** planted defects detected |
| `python scripts/check_contrast.py` | Pass — all 22 DESIGN.md contrast pairs recomputed and matched; formula self-validated against 3 WCAG reference pairs |

**Every gate has been seen to fail, not just pass.** `cargo clippy` denies `unwrap()`
(exit 101); `cargo deny` reports `RUSTSEC-2020-0071` on a seeded vulnerable crate
(exit 1); the traceability audit catches 5 of 5 planted defects. Details in
`docs/dependency-register.md`.

Known environment gaps: there is **no Windows SDK**, so the MSVC toolchain can
compile but not link; and the GNU toolchain has **no `dlltool.exe`**. Neither can
compile Rust tooling from source — which is why the prebuilt `cargo-deny` binary
is used above.

Also verified: the lint gate is not decorative. A temporary `unwrap()` failed
clippy with exit 101 and passed again on revert.

**Not yet valid:** CI has never run — no remote or CI provider is configured. The
planned gates (TEST-007) are formatting, lint, tests, dependency scan, and applying
all migrations to an empty database. Planned suites live under `tests/`
(integration, game-compat, network, security, perf). **No application test has been
run, because there is no application code.**

What *has* been checked in the documentation set:

- All **11 Mermaid diagrams render** with `@mermaid-js/mermaid-cli` **12.0.0** (2026-10-17), including the container diagram edited for the server-manager component. A deliberately malformed diagram was rejected with a parse error and produced no output, so the renderer is confirmed to fail on bad input rather than passing everything. Syntax and rendering only — **visual layout was not reviewed**.
- The traceability audit passes and catches **10 of 10** planted defects, including two that verify every ADR in `research.md` §7 has a summary-table row — added after ADR-016 was found to have a full record but no row, while ADR-017 had both.
- **`data-model.md` and `schema-draft.sql` are compared mechanically** — all 12 tables, column by column. Added after `T-120` was found to name this as a standing gap.
- **Colour contrast is recomputed, not asserted.** `scripts/check_contrast.py` re-derives all 22 ratios in `DESIGN.md` §3 from the token table in §2, after validating its own implementation against three WCAG reference pairs. All 22 matched, and it was confirmed to fail on a planted overstated ratio and a planted wrong threshold. Hover, disabled, and high-contrast states are still unchecked.
- **The database schema has been executed**, not merely parser-checked: applied cleanly to an empty database on **PostgreSQL 16.10 and 17.6**, producing 12 tables, 35 indexes, 57 constraints and 13 foreign keys with zero errors. PRIV-006 (no email column) and INV-1 (no local destination column) were checked against the live schema, and four negative tests confirmed the CHECK constraints reject bad input. Method, version matrix, and one environment limitation are recorded in `schema.md`.

**Withdrawn:** the earlier claim that "11 JSON Schemas are valid and example payloads were checked against them". `contracts/` does not exist in this repository, so no schema validation has been performed here. See `contracts.md`.

## Configuration example

No configuration files exist yet. See the illustrative variables above.

## Troubleshooting

Application troubleshooting will be documented when there is an application. For documentation work: if two documents disagree, follow the source-of-truth table in `AGENTS.md` §2 and report the conflict.

## Documentation map

| Document | Purpose |
|---|---|
| `context.md`, `idea.md` | Product context, scope, brief |
| `research.md` | Evidence, experiments, ADRs |
| `architecture.md` | System architecture |
| `data-model.md`, `schema.md`, `schema-draft.sql` | Data model and schema |
| `contracts.md`, `api.md`, `contracts/` | API and protocol contracts, JSON Schemas |
| `requirements.md`, `spec.md` | Requirements and index |
| `tasks.md`, `plan.md` | Implementation checklist and execution plan |
| `user-flows.md`, `ux.md`, `DESIGN.md` | Flows, UX principles, design system |
| `AGENTS.md`, `CLAUDE.md` | Instructions for AI coding agents |
| `review-report.md` | Planning-phase review: coverage, audit results, blockers |
