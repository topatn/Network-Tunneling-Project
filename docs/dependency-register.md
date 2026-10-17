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
| `cargo-deny` | Licence/advisory/bans checking (SEC-017) | Dev dependency; policy file at `deny.toml` |

## How to prove the scan actually fails (T-001 verification)

**NOT DONE — and blocked by the environment, not by the project.** Recorded here
so the gap is visible rather than forgotten.

What was attempted on 2026-10-17, and what happened:

| Step | Result |
|---|---|
| `cargo install cargo-deny --locked` on `stable-x86_64-pc-windows-msvc` | **Failed.** `rustup` warned the MSVC toolchain was installed without prerequisites. The MSVC compiler (14.51.36231) is present, but there is **no Windows SDK** (`Windows Kits\10\Lib` absent), so `kernel32.lib` is missing and every crate **build script** fails to link. |
| `cargo install cargo-deny --locked` on `stable-x86_64-pc-windows-gnu` | **Failed.** Links for plain crates, but `dlltool.exe` is not present, so `getrandom` and `windows-sys` fail. rustup's GNU toolchain does not ship MinGW binutils. |
| Install the Windows SDK or MinGW-w64 | **Not attempted.** Both are system software, which `AGENTS.md` §9.6 forbids installing without asking. **Owner decision required.** |

Consequence: the `dependency-scan` CI job and this whole file's enforcement story
are **unproven**. `deny.toml` has never been parsed by the real `cargo-deny`
binary, so even its syntax is unconfirmed.

The procedure to run once a working `cargo-deny` exists:

1. Add a crate with a known advisory to the **workspace** as a temporary dev
   dependency, and record it in this file marked "seeded for T-001 verification".
2. Run `cargo deny check advisories`.
3. Confirm the job **fails** and names the advisory.
4. Remove the crate and this row in the same change.

The point is to show the gate is not decorative. **A scan that has never been
seen to fail is not evidence of anything**, and right now that is exactly the
state of the dependency gate.

## What *was* verified locally (2026-10-17)

Toolchain: `rustc 1.99.1`, `cargo 1.99.1`, `clippy 0.1.99`, `rustfmt 1.10.0`.

| Check | Result |
|---|---|
| `cargo metadata --no-deps` | Pass — 6 members, empty dependency graph |
| `cargo check --workspace` | Pass |
| `cargo fmt --all -- --check` | Pass (after fixing generated trailing blank lines) |
| `cargo clippy --workspace --all-targets -- -D warnings` | Pass, zero warnings |
| Lint gate actually denies `unwrap()` | **Pass** — a temporary `unwrap()` produced `error: used unwrap() on an Option value`, exit 101, and clippy returned to clean on revert |
| `cargo test --workspace --all-targets` | Pass under `stable-x86_64-pc-windows-gnu` — **0 tests**, because the crates are empty skeletons |

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