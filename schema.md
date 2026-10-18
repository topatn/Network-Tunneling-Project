# schema.md — Schema Reference and Migration Workflow

**Status:** DRAFT. Companion to `data-model.md`; contains no table definitions of its own.

## Which source is authoritative

| Phase | Authoritative for | Source |
|---|---|---|
| Completed | Entities, constraints, invariants, lifecycle | `data-model.md` |
| **Current (from 2026-10-18)** | **Physical schema** | **`migrations/`** |

Rules:
1. If `data-model.md` and the DDL disagree, **`data-model.md` wins** and the SQL is corrected in the same change.
2. `migrations/` is authoritative for the physical schema. **Satisfied on 2026-10-18 (T-033):** the first migration exists and `schema-draft.sql` was deleted in the same change, as rule 2 requires. `data-model.md` is updated in the same pull request as any schema migration.
3. There must be no second DDL definition anywhere else (not in README, not in API docs).
4. A migration that has been applied anywhere is never edited. Fix with a new forward migration.

`scripts/gen_initial_migration.py` generated the first migration from `schema-draft.sql` rather than by transcription, so the DDL that had been verified against PostgreSQL 16.10 and 17.6 is byte-faithful in `migrations/`. That script is **run once, by hand**. It is not part of any workflow and must not be re-run: it refuses to overwrite an existing migration.

## `migrations/` contents

| File | Purpose |
|---|---|
| `20261018120000_initial_schema.up.sql` | The whole initial schema: 12 tables, 35 indexes, 57 constraints |
| `20261018120000_initial_schema.down.sql` | Reverse-creation-order drops, for local and test teardown only |

Both use the `sqlx` reversible naming convention (`<timestamp>_<name>.up.sql` / `.down.sql`). One migration for a greenfield schema is deliberate; splitting it by logical group would create drop-order coupling for no benefit. **Every foreign key here is declared inline at table creation**, which is what makes reverse creation order a valid drop order for the `.down.sql`. That is a property of this schema, not a general rule — a later migration adding an out-of-line `ALTER TABLE ... ADD CONSTRAINT` must not copy the assumption.

### What is verified about these files, and how

Verified on **2026-10-18** by applying `20261018120000_initial_schema.up.sql` to an empty cluster:

| Claim | Method | Result |
|---|---|---|
| Applies cleanly | Applied to an empty cluster, errors inspected | 0 errors on 16.10 and 17.6 |
| Structure is as documented | Counted from `pg_constraint`, public schema | 12 tables, 35 indexes, 57 constraints (13 FK / 23 CHECK / 9 UNIQUE / 12 PK), 0 FKs on `audit_events` |
| Constraints are **enforced** | Six statements executed; four bad inserts must fail, two good ones must succeed | All six behaved as required, each rejecting with the intended constraint name |
| Re-running is safe | `sqlx migrate run` twice more in CI | No-op |
| The `.down.sql` runs | Applied up, then down, then up again on a real 16.10 cluster | 12 tables → **0** → 12, zero errors |
| `gen_random_uuid()` needs no extension | Counted matching `pg_proc` entries | 1; no `CREATE EXTENSION` anywhere |
| PRIV-006 | Columns matching `%email%` | 0 |
| INV-1 | Columns matching `%local_%` or `%target_%` | 0 |

The counts are **identical on 16.10 and 17.6**, so the CI job can assert a single set of values across the matrix.

### Read constraint counts from `pg_constraint`, not `information_schema`

While setting up the CI assertions, `information_schema.table_constraints` was tried first and produced **102 CHECK constraints** where `pg_constraint` produces **23**, and **611** with no schema filter at all. The `pg_constraint` figures are the correct ones: their total, 57, matches the value independently measured on 2026-10-17 and recorded in `data-model.md`.

`pg_constraint` is the system catalog and is the source used by the CI assertions. Anyone re-deriving these numbers must use it too. This is recorded because the failure mode is silent — the information_schema view returns a confident, well-formed, wrong number rather than an error.

## Current verification status

## Current verification status

**Updated 2026-10-18 — a real migration set now exists and CI applies it.** T-033's verification ran; see the matrix below.

- `migrations/20261018120000_initial_schema.up.sql` **applies cleanly to an empty database on PostgreSQL 16.10 and 17.6**, producing identical structure on both: **12 tables, 35 indexes, 57 constraints, 13 foreign keys**, with **zero errors**. `gen_random_uuid()` works out of the box on both, so the extension concern is resolved — it is built in from PostgreSQL 13 onward and needs no `CREATE EXTENSION`.
- **PRIV-006 verified:** a query for any column matching `%email%` across all tables returns **0 rows** on both versions.
- **INV-1 verified on 16.10 and 17.6.** No column matches `%local%`, `%bind%`, `%dest%`, `%listen%`, or `%target%`. The three `%port%` columns are all legitimate: `relay_nodes.port_range_start`, `relay_nodes.port_range_end`, `allocation_ports.public_port` — these are the **relay's** public ports, not a host-local destination, which is exactly the distinction INV-1 draws. The two address-like columns are `guest_admissions.source_ip` (`inet`, the relay-enforced admission set) and `relay_nodes.public_host` (`text`, the relay's own hostname). The single `inet` column is the admission source IP, which `data-model.md` §1.4 explicitly permits.
- `audit_events` has **0 foreign keys**, confirming the deliberate design in `data-model.md` §3.
- **Negative tests pass**, now on every push through a normal client connection rather than only through `--single`. Four bad inserts are rejected by `users_status_chk`, `tunnels_profile_version_chk`, `tunnels_expiry_chk`, and `tunnels_stopped_consistency_chk`; two valid inserts are accepted.

### Finding: `postgres --single` is line-oriented, and this recurred

The first attempt reported `relation "idempotency_keys" does not exist` for its index, and no tables existed afterwards. The cause was the test harness, not the schema: `postgres --single` submits a statement at each newline, so a multi-line `CREATE TABLE` was split and every column line was parsed as its own query. All 26 statements were re-emitted one per line and then applied cleanly. **This is a harness defect, and it is recorded because the same class of failure would silently produce a partial schema in any line-oriented runner.**

**It happened a second time on 2026-10-18.** Producing the CI assertions required applying the migration locally again, and the first attempt split the file on `;` and rejoined with newlines — which reconstitutes exactly the defect the section above documents, and produced 200-plus syntax errors and a zero-table schema. The recovery was `scripts/apply_via_single.py`, which collapses each statement onto one line before submission and is now the only supported way to execute DDL locally.

Two rules follow, and both are permanent:

1. **Never write ad-hoc `--single` input.** Use `scripts/apply_via_single.py`. It prints the statement count it submitted and fails on any `ERROR`/`FATAL` in the server output.
2. **A line-oriented runner cannot be trusted with multi-line DDL.** Real migration tooling (`sqlx migrate`, `refinery`) parses statement boundaries properly. The CI job confirms `sqlx migrate` does rather than assuming it, which is why it runs even though the local path now works.

A third trap, found in the same session and now handled: `postgres --single` writes results to stdout in a tabular form that is awkward to parse, so labelling a query by prefixing the label into the SQL (`tables|select ...`) is a syntax error. Label in the query instead — `select 'tables=' || count(*) ...`.

### Version matrix

| Version | Status | Result |
|---|---|---|
| PostgreSQL 16.10 | Verified 2026-10-17 (draft), re-verified 2026-10-18 (migration) | Applies cleanly; 12 tables, 35 indexes, 57 constraints, 13 FKs; 0 email columns; 4 negative tests rejected, 2 positive accepted |
| PostgreSQL 17.6 | Verified 2026-10-17 (draft), re-verified 2026-10-18 (migration) | Identical result to 16.10, including every count |
| 14.x and earlier | **Not tested** | Out of scope: `gen_random_uuid()` requires PostgreSQL 13+, and the project targets current releases. Re-test if support is ever widened |
| 18.x | **Not tested** | Not available at time of writing. Should be added before release |

**Recommended supported range: PostgreSQL 16 to 17.** Both ends are verified; anything outside is untested and must not be claimed.

### Method, and an environment limitation that matters

The schema was applied with the **official EDB standalone binaries**, extracted to a temporary directory. **No system installation was performed**: no service, no registry changes, no Program Files entry, no `winget` install. Everything is under the temporary directory and can be deleted outright.

**Limitation, recorded honestly:** a normal client connection cannot be used on the authoring machine. The server starts and accepts connections, but forked backends die with `STATUS_DLL_INIT_FAILED` (0xC0000142) in this environment, so `psql` connections terminate unexpectedly. **All local verification therefore runs through `postgres --single`, which executes in a single process and never forks.** What this means:

- **Verified locally:** the DDL parses and executes, all objects are created, defaults work, constraints and foreign keys are enforced, and the negative tests genuinely fail as designed.
- **Not verified locally:** anything requiring a normal client session — concurrent access, transaction isolation under load, connection handling, `query` behaviour, and live performance.

This is an environment limitation, not a property of PostgreSQL or of the schema. A normal CI runner or developer machine will not hit it.

**This limitation is now closed for the migration path.** The `migrate` job in `.github/workflows/ci.yml` runs `sqlx migrate` against a real PostgreSQL service container over a normal client connection, on both 16 and 17, on every push. That is the first verification of this schema through the real client path, and it also confirms that `sqlx migrate` parses statement boundaries correctly — the specific risk this environment could not test.

**What CI still does not cover, and cannot:** concurrency, transaction isolation under load, and live performance. Those need a workload, not a schema. They remain open and are not claimed.

## Migration workflow

1. **Generate:** write a new numbered SQL file in `migrations/`; never edit an applied one.
2. **Review:** pull request reviewed against the compatibility rules in `data-model.md` §8; a migration touching a large table needs a stated plan.
3. **Apply:** one migration step per deployment, run before new application code is started; staging first.
4. **Roll back:** deploy a compensating forward migration. Destructive steps (drop column/table) happen only in a later release after code stops using them. The `.down.sql` files exist for local and test teardown, **not** for production rollback.
5. **Verify:** CI applies all migrations to an empty database and checks the result against the expected structure.

The migration tool is **`sqlx migrate`** (ADR-017, accepted 2026-10-17). The driver is already `sqlx` (ADR-008), so this adds no further dependency. Step 5 is implemented: the `migrate` job in `ci.yml` installs `sqlx-cli`, applies every migration, applies them twice more to prove idempotency, then asserts the structure against the measured values in the table above and runs the negative tests.

**`sqlx-cli` cannot be executed on the authoring machine** — `dlltool.exe` is missing from the GNU toolchain, the MSVC toolchain has no Windows SDK, and no prebuilt Windows binary is published. It is installed and run **on the CI runner**, which is why this task became possible at all: the local environment could not verify statement-boundary parsing, and a hosted Linux runner has neither limitation. Do not claim the local path verifies this; only CI does.
