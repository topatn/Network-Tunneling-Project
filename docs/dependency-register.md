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

Every crate below is a **direct** dependency. The transitive tree is governed by `deny.toml`
and enforced by `cargo deny`, which is what makes that sufficient; enumerating 167
transitive rows here would be a list nobody maintains and would not be a control.

| Crate | Version | Purpose | Licence | Maintenance note | Added by |
|---|---|---|---|---|---|
| `sqlx` | 0.9.0 | PostgreSQL driver for the persistence layer, and `sqlx migrate` (ADR-008, ADR-017) | Apache-2.0 | Repo pushed 2026-10-16, not archived. `rust-version = 1.94.0`, which is why the workspace MSRV is 1.94 rather than 1.77. Added with **`default-features = false`** and only `postgres`, `runtime-tokio`, `uuid`, `chrono`, `json`: the defaults pull `sqlite`, `mysql` and `native-tls`, none of which this project uses. **`macros` and `derive` are both deliberately off** — see below | T-034 |
| `tokio` | 1.53.3 | Async runtime. A **dev-dependency of `gtt-control-plane` only** for now | MIT | Repo pushed 2026-10-16, not archived. Will move to `[dependencies]` when the HTTP server exists (T-040); until then it would be a dependency the library does not use | T-034 |
| `uuid` | 1.28.0 | Tunnel, user, session and allocation identifiers | MIT OR Apache-2.0 | Active; a dependency of `sqlx`, so it would be in the tree regardless | T-034 |
| `chrono` | 0.4.45 | `timestamptz` columns | MIT OR Apache-2.0 | Active; a dependency of `sqlx`, so it would be in the tree regardless | T-034 |
| `serde` | 1.0.229 | Derives, and `jsonb` binding via `sqlx`'s `json` feature | MIT OR Apache-2.0 | Active | T-034 |
| `serde_json` | 1.0.154 | `jsonb` values: `audit_events.metadata`, `idempotency_keys.response_body` | MIT OR Apache-2.0 | Active | T-034 |

### Why `sqlx`'s `macros` and `derive` features are off

Both were tried and both were reverted. Recorded because the reasoning is not obvious and
the default is to turn them on.

- **`macros`** provides `query!` and `query_as!`, which type-check SQL at compile time.
  That requires either a live database or a checked-in `.sqlx` cache of query metadata
  (`sqlx-core/offline`). No normal PostgreSQL client connection is possible in the authoring
  environment (`schema.md`), so enabling it would make `cargo check` fail there. The
  runtime-checked `sqlx::query` / `query_as` API is used throughout instead. **Revisit when
  `.sqlx` offline data can be generated** — that is a follow-up, not a permanent answer.
- **`derive`** provides `#[derive(sqlx::FromRow)]`. It pulls in `sqlx-macros`, a proc-macro
  crate, and a proc macro must be **linked** to a dylib. This machine cannot link
  anything: the GNU bundle ships no assembler so `dlltool` cannot spawn one, and the MSVC
  toolchain has no Windows SDK so `link.exe` does not exist. Verified, not assumed — see
  the environment section below. Enabling it would have cost `cargo check`, `cargo clippy`
  and `cargo test` for the whole crate locally, in exchange for removing about 150 lines of
  boilerplate. `sqlx::FromRow` the *trait* needs no feature, so
  `crates/control-plane/src/db/entities.rs` generates the impls with a `macro_rules!` macro
  instead. CI can build proc macros either way; this was a local-verification decision.

### What the real graph costs, measured

| Measurement | Result |
|---|---|
| `[[package]]` entries in `Cargo.lock` after adding these six | **167** (the register previously measured 145 for `sqlx` with its default features) |
| `cargo deny check advisories bans licenses` on that graph | **Pass, exit 0** — advisories ok, bans ok, licences ok |
| Duplicate versions | 7 pairs warned (`block-buffer`, `cpufeatures`, `crypto-common`, `digest`, `hashbrown`, `sha2`, `syn`), advisory-only under `multiple-versions = "warn"` |
| Workspace MSRV | Raised 1.77 → **1.94** by `sqlx`. `clippy.toml` said 1.77 until 2026-10-18, so clippy had been linting against the wrong MSRV since ADR-008; corrected |

**Status 2026-10-17 (superseded):** the workspace had **no dependencies**. All six crate
manifests declared an empty `[dependencies]` block on purpose — adding a placeholder crate
to prove the scan works would have put an unreviewed dependency in the tree, which §5.1
forbids. **Superseded 2026-10-18 by T-034**, which needed `sqlx` to do real work.

## Planned dependencies (not yet added — each needs a row here first)

`sqlx`, `tokio`, `serde` and `serde_json` were planned here and are now **added**; see the table above. They remain on this list only for the parts not yet reached.

| Crate | For | Status |
|---|---|---|
| `quinn` | QUIC with datagram support for engine and relay (ADR-001) | Proposed; RQ-6 confidence medium on datagrams, low on production fitness until E-6 |
| `axum` | Control-plane HTTP API (ADR-008) | **0.8.9, MIT, repo pushed 2026-10-15, not archived, 27.5k stars. Accepted by ADR-008 on 2026-10-17** |
| `tracing`, `tracing-subscriber` | Structured logging (ADR-010) | `tracing` 0.1.44, MIT |
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

### The Windows SDK could not be installed, and `dlltool` turned out not to be the fix (2026-10-17)

I recommended the Windows SDK as "the single highest-value environment fix". **Attempted, and it is blocked.**

| Attempt | Outcome |
|---|---|
| `winget search "Windows SDK"` | Only **10.0.17134** (2019) and **10.0.18362** (2019) offered. No current SDK in the winget source |
| `winget install` either | **Not attempted.** Both candidates are six years old, and installing a 2019 SDK alongside MSVC 14.51 is a likely-to-break pairing |
| Adding an SDK component to the existing **Visual Studio Community 2026** (18.10.12210.168) | **Requires elevation.** This session is **not** elevated (`IsInRole(Administrator)` = false). `AGENTS.md` §9.5 forbids escalating without an explicit instruction |
| VS already has a Windows SDK? | No. `vswhere` reports no `Microsoft.VisualStudio.Component.Windows11SDK.*`, and only `Windows Kits\NETFXSDK` exists — **no `Lib` directory at all**, which is why `kernel32.lib` is missing |

**A more promising lead, which did not pan out.** `dlltool.exe` is *not* actually missing — rustup ships a self-contained MinGW bin at `~/.rustup/toolchains/stable-x86_64-pc-windows-gnu/lib/rustlib/x86_64-pc-windows-gnu/bin/self-contained/`, containing `dlltool.exe` (GNU Binutils 2.44), `ld.exe`, and `gcc.exe`. Adding it to `PATH` moved the failure along:

```
dlltool could not create import library with ...\self-contained\dlltool.exe ... : CreateProcess
```

`dlltool` runs fine when invoked directly, but fails to spawn the assembler it shells out to, because the self-contained bundle ships no `as.exe`. **So the GNU toolchain's missing piece is the assembler, not `dlltool`**, and the self-contained bundle cannot substitute for a full MinGW-w64 installation.

**Conclusion, recorded so it is not retried blindly.** To compile Rust tooling from source, this machine needs **one** of: the Windows SDK (admin, ~1–2 GB, for MSVC), a full MinGW-w64 installation (admin, ~100 MB, for GNU), or a Linux/CI runner for builds. All three need elevation, which this session does not have. **This is an owner action, not an agent action**, and it is now the one blocker standing between "all gates verified locally" and "tooling can be compiled at all".

What this does *not* block: `cargo check`, `cargo clippy`, `cargo fmt`, `cargo deny` (prebuilt binary), and PostgreSQL verification via `--single`.

> **Superseded 2026-10-18.** This paragraph listed `cargo test` (GNU, linking via `ld.exe`)
> as unaffected. That was true only while the workspace had no dependencies. With `sqlx`
> added, linking anything at all fails, and `cargo test` no longer runs here. See "What
> changed on 2026-10-18" above. The gates that still pass locally are listed there
> accurately; the ones that do not are named.

For T-003's migration-tool research, `sqlx 0.9.0` was added to `gtt-control-plane` temporarily to measure what it costs and whether it builds here. **Removed in the same change; the workspace is back to zero dependencies and `git status` is clean.**

What it showed, recorded because the numbers matter for the ADR:

- **`sqlx` resolves 145 transitive crates.** That is the cost of the `sqlx-cli` tool plus its query layer.
- **It does not build on the MSVC toolchain** — `link.exe` missing, no Windows SDK, so `quote`, `proc-macro2` and other build scripts fail to link.
- **It does build on `stable-x86_64-pc-windows-gnu`**, which needs no SDK. Note that toolchain ships **without** `clippy` and `rustfmt`; add them with `rustup component add clippy rustfmt --toolchain stable-x86_64-pc-windows-gnu`, otherwise `cargo clippy` fails with a misleading "not installed for the toolchain" error.
- **`cargo deny` passes on the real 145-crate graph** — advisories ok, bans ok, licences ok — with seven `duplicate` warnings (two versions of `block-buffer`, `cpufeatures`, `crypto-common`, `digest`, `hashbrown`, `sha2`, `syn`). Those are advisory-only under `multiple-versions = "warn"`, but the duplicate-version signal becomes real as soon as a substantial dependency exists, so the policy needs revisiting when T-030 picks dependencies.
- **`sqlx` declares `rust-version = "1.94.0"`**, above this repo's declared 1.77, which forces a decision about the workspace MSRV.
- The dead-code lint proved itself here: the temporary probe module was rejected by `clippy -D warnings` with `function 'probe' is never used`. The lint gate works on real code, not only on the synthetic `unwrap()` test.

## What changed on 2026-10-18: `cargo test` no longer runs locally

**This supersedes the claim above that `cargo test` works under the GNU toolchain.** That
was true only while the workspace had **no dependencies** — with nothing to link, there
was nothing to link *with*. Adding `sqlx` on 2026-10-18 changed it.

| Check | Before `sqlx` | After `sqlx` |
|---|---|---|
| `cargo check --workspace --all-targets` | Pass | **Pass** — `check` does not link |
| `cargo clippy --workspace --all-targets` | Pass | **Pass** — clippy does not link |
| `cargo fmt --all -- --check` | Pass | **Pass** |
| `cargo test --workspace` | Pass (0 tests) | **Cannot run** |

The failure is reproducible and is not a configuration mistake:

```
error: error calling dlltool 'dlltool.exe': program not found
error: could not compile `getrandom` (lib) due to 1 previous error
```

Adding the self-contained bundle to `PATH` (as the register's earlier note recommends) moves
the failure along, exactly as that note predicts, and no further:

```
dlltool could not create import library with ...\self-contained\dlltool.exe ... : CreateProcess
```

`dlltool.exe` runs (`GNU Binutils 2.44`) but cannot spawn the assembler it shells out to.
Confirmed there is **no `as.exe` anywhere under `~/.rustup`**, and `link.exe` does not exist
for the MSVC side.

**Consequence, and it is a real loss.** The persistence layer's test suite and its
clippy-clean, compiling state can be verified locally, but **the tests cannot be executed
here.** They execute in CI: the `db-tests` job, which is also the only place the
database-backed tests can run at all, since a normal client connection is impossible here
either. This is recorded rather than worked around, because the workaround would be to
stop writing tests that need a database, which is the opposite of correct.

Nothing in this section is an owner action that would help. A Windows SDK, a full MinGW-w64,
or a Linux runner all fix it; the first two need elevation and the third is what CI already
is.

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
- Whether `Cargo.lock` is committed: **it is**, and it should stay committed, for a
  binary-producing workspace, so builds are reproducible. `Cargo.lock` was added to the
  repository on 2026-10-17 (6 crates, the workspace itself).
- Licence compatibility review is blocked on the project's own licence decision.
  `deny.toml` currently ignores private crates, so this remains deferred deliberately.
- **ADR-008 was Accepted 2026-10-17** (Rust control plane with `axum`, PostgreSQL via
  `sqlx`), and **ADR-017 the same day** (`sqlx migrate` as the migration tool, adding no
  further dependency). `axum`, `tokio`, and `sqlx` still need real `Cargo.toml` entries
  in T-030.
- **No Rust tooling can be compiled from source in this environment.** The MSVC
  toolchain has no Windows SDK, and the GNU toolchain's self-contained MinGW bundle
  ships no assembler (`as.exe`), so `dlltool` cannot spawn one. This blocked
  `cargo-deny` (worked around with the official prebuilt binary) and blocks
  `sqlx-cli`, which publishes no prebuilt Windows binary. **Fixing this needs
  elevation** — the Windows SDK (~1–2 GB), MinGW-w64 (~100 MB), or building on a
  Linux/CI runner. This session is not elevated, so it is an owner action. See the
  attempt log in the "Windows SDK could not be installed" section.
- PostgreSQL 16.10 and 17.6 **were** run successfully from EDB standalone binaries in a
  temporary directory, so the schema itself is verified. No system install was made.