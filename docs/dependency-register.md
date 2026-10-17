# Dependency register

**Purpose.** Every dependency in `Cargo.lock` must appear here with a purpose, a
licence, and a maintenance note. This file is the evidence for **SEC-017** and for
the dependency policy in `AGENTS.md` §5. The CI vulnerability scan blocks a merge
on a known-vulnerable dependency above a configured severity; suppressing a
finding requires a written justification **in this file**, in the same change.

**Rules.**
1. Add a row in the same change that adds the dependency. No exceptions.
2. Pin versions. Prefer releases that have been public for a while and whose
   changelog you have read.
3. Record the licence and whether it is compatible with the project's own
   licensing decision (`Cargo.toml` currently says `UNLICENSED`, provisional).
4. Do not run install scripts from untrusted sources, and do not pipe downloaded
   scripts into a shell.
5. A removal gets a row too, with the reason and the date.

## Rust dependencies

| Crate | Version | Purpose | Licence | Maintenance note | Added by |
|---|---|---|---|---|---|
| _(none yet)_ | | | | | |

**Status 2026-10-17:** the workspace has **no dependencies**. All six crate
manifests declare an empty `[dependencies]` block on purpose. Adding a
placeholder crate to prove the scan works would put an unreviewed dependency in
the tree, which §5.1 forbids.

## Planned dependencies (not yet added — each needs a row here first)

| Crate | For | Status |
|---|---|---|
| `quinn` | QUIC with datagram support for engine and relay (ADR-001) | Proposed; RQ-6 confidence medium on datagrams, low on production fitness until E-6 |
| `serde`, `serde_json` | Protocol message encoding (contracts.md §10) | Proposed |
| `tokio` | Async runtime for engine, relay, control plane | Proposed |
| `axum` | Control-plane HTTP API (ADR-008) | Proposed — **ADR-008 confidence is low; backend stack is reasoning only** |
| `sqlx` or `postgres` | PostgreSQL access | Proposed — choice deferred to T-003/T-030 |
| `tracing`, `tracing-subscriber` | Structured logging (ADR-010) | Proposed |
| `keyring` | OS credential store (SEC-002) | Proposed |
| `cargo-deny` | Licences, advisories, and bans (SEC-017) | Dev/CI tool; policy file at `deny.toml`. **Use the official prebuilt binary**, not `cargo install` — compiling it needs a linker this environment lacks (see the T-001 verification section) |

## How to prove the scan actually fails (T-001 verification)

**DONE 2026-10-17.** The gate has now been observed to fail on a seeded known-vulnerable dependency, and to pass again once removed.

Approach: rather than installing a ~1–2 GB Windows SDK to compile `cargo-deny` from
source, the **official prebuilt binary** was used — the documented distribution
channel for this tool.

| Step | Result |
|---|---|
| Resolve the latest release via the GitHub API | `0.20.2` |
| Download `cargo-deny-0.20.2-x86_64-pc-windows-msvc.tar.gz` + its `.sha256` | Downloaded |
| Verify SHA-256 against the published checksum | **Match** (`975a2214…cf87dc`) |
| Extract and run `cargo-deny --version` | `cargo-deny 0.20.2` |
| `check advisories bans licenses` on the empty workspace | **Pass, exit 0** |
| Seed `time = "0.1.45"` into `crates/protocol` (RUSTSEC-2020-0071) | — |
| `check advisories` with the seed present | **FAIL, exit 1** — `error[vulnerability]: Potential segfault in the time crate`, `ID: RUSTSEC-2020-0071`, `advisories FAILED` |
| Remove the seed, re-run | **Pass, exit 0** |

**The gate is real.** It is not decorative, and it blocks a merge on a known advisory.

Two process notes, because both nearly produced a false pass:

1. **Seeding `[workspace.dependencies]` is not seeding the graph.** Adding `time`
   there declares it for *possible* use; no crate depends on it, so the scan
   correctly reported `advisories ok` and exit 0. The seed must go into an actual
   crate's `[dependencies]`. A "successful" test that never put the crate in the
   graph would have been worthless.
2. **`cargo-deny` shells out to `cargo`**, so cargo must be on `PATH` or the tool
   fails with a bare `program not found`.

### Configuration errors found and fixed by actually running it

`deny.toml` had never been parsed by a real `cargo-deny`, and it was wrong in three
ways. All three are fixed:

- **`[licenses] deny = [...]` was removed in this release** and made the entire
  config fail validation. Rejection is now by omission: anything not in `allow`
  fails. The copyleft entries were deleted from the config accordingly.
- **An empty dependency graph fails the licences check**, because every allowance
  is "unmatched". Set `unused-allowed-license = "warn"`, which stays meaningful once
  dependencies exist.
- **Our own crates are `license = "UNLICENSED"`**, which is a Cargo convention and
  not a valid SPDX expression, so each crate was reported `error[unlicensed]`. All
  workspace crates are `publish = false`, so `[licenses.private] ignore = true` is
  the correct treatment. This deliberately **defers** the project's own licensing
  decision rather than inventing one.

The toolchain gap remains for *compiling* Rust tooling, but it is no longer what
stands between this repository and a proven dependency gate.

## What *was* verified locally (2026-10-17)

Toolchain: `rustc 1.99.1`, `cargo 1.99.1`, `clippy 0.1.99`, `rustfmt 1.10.0`. Dependency
gate: `cargo-deny 0.20.2` (official prebuilt Windows binary, checksum verified).

| Check | Result |
|---|---|
| `cargo metadata --no-deps` | Pass — 6 members, empty dependency graph |
| `cargo check --workspace` | Pass |
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | Pass, zero warnings |
| Lint gate actually denies `unwrap()` | **Pass** — a temporary `unwrap()` produced `error: used unwrap() on an Option value`, exit 101, and clippy returned to clean on revert |
| `cargo test --workspace --all-targets` | Pass under `stable-x86_64-pc-windows-gnu` — **0 tests**, because the crates are empty skeletons |
| `cargo deny check advisories bans licenses` | **Pass, exit 0** |
| Dependency gate fails on a known advisory | **Pass** — seeded `time 0.1.45`, got `RUSTSEC-2020-0071`, exit 1; removed, back to exit 0 |
| `scripts/audit_traceability.py` | **Pass** — 110 requirements, 110 traceability rows, 86 tasks, 0 errors, 7 warnings |
| `scripts/audit_traceability.py --self-test` | **Pass** — 5 planted defects, 5 detected |

Two configuration bugs were caught by running the checks rather than trusting
them, and are worth remembering:

1. `unsafe_code` under `[lints.rust]` in **`clippy.toml`** does not compile —
   `clippy.toml` accepts clippy lint names only. Lint policy now lives in the
   root `Cargo.toml` under `[workspace.lints]`, and crates opt in with
   `[lints] workspace = true`.
2. `unwrapping_used` and `expecting_used` are **not real lint names** (they are
   `unwrap_used` and `expect_used`). Unknown lints only produce a *warning*, and
   `unknown_lints` ignores `-D warnings`, so the policy would have looked
   configured while enforcing nothing. This is the same failure mode as an
   unverified CI gate, and is exactly why the deny check above matters.

## Open items

- Identity provider and admin authentication: unreached, ADR-009 and SEC-018 open.
- Whether `Cargo.lock` is committed: **it must be**, for a binary-producing
  workspace, so builds are reproducible. Deferred to T-030.
- Licence compatibility review is blocked on the project's own licence decision.