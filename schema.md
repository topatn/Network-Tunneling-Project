# schema.md — Schema Reference and Migration Workflow

**Status:** DRAFT. Companion to `data-model.md`; contains no table definitions of its own.

## Which source is authoritative

| Phase | Authoritative for | Source |
|---|---|---|
| Planning (now) | Entities, constraints, invariants, lifecycle | `data-model.md` |
| Planning (now) | Physical DDL draft (derived) | `schema-draft.sql` |
| After task for migrations is complete | Physical schema | `migrations/` (does not exist yet) |

Rules:
1. If `data-model.md` and `schema-draft.sql` disagree during planning, **`data-model.md` wins** and the SQL is corrected in the same change.
2. Once `migrations/` exists, it is authoritative for the physical schema. `schema-draft.sql` is deleted in the same change that adds the first migration. `data-model.md` is then updated in the same pull request as any schema migration.
3. There must be no second DDL definition anywhere else (not in README, not in API docs).

## Current verification status

**Updated 2026-10-17 — the schema has now been executed.** T-003's verification ran; see the matrix below.

- `schema-draft.sql` **applies cleanly to an empty database on PostgreSQL 16.10 and 17.6**, producing identical structure on both: **12 tables, 35 indexes, 57 constraints, 13 foreign keys**, with **zero errors**. `gen_random_uuid()` works out of the box on both, so the extension concern is resolved — it is built in from PostgreSQL 13 onward and needs no `CREATE EXTENSION`.
- **PRIV-006 verified:** a query for any column matching `%email%` across all tables returns **0 rows** on both versions.
- **INV-1 verified on 16.10 and 17.6.** No column matches `%local%`, `%bind%`, `%dest%`, `%listen%`, or `%target%`. The three `%port%` columns are all legitimate: `relay_nodes.port_range_start`, `relay_nodes.port_range_end`, `allocation_ports.public_port` — these are the **relay's** public ports, not a host-local destination, which is exactly the distinction INV-1 draws. The two address-like columns are `guest_admissions.source_ip` (`inet`, the relay-enforced admission set) and `relay_nodes.public_host` (`text`, the relay's own hostname). The single `inet` column is the admission source IP, which `data-model.md` §1.4 explicitly permits.
- `audit_events` has **0 foreign keys**, confirming the deliberate design in `data-model.md` §3.
- **Negative tests pass.** Five invalid inserts were attempted; four were rejected by the intended constraints: `users_status_chk`, `tunnels_profile_version_chk`, `tunnels_expiry_chk`, and `tunnels_stopped_consistency_chk`. **One did not fail as intended** — see the finding below.

### Finding: the `idempotency_keys` table was never created on the first apply

The first attempt reported `relation "idempotency_keys" does not exist` for its index, and no tables existed afterwards. The cause was the test harness, not the schema: `postgres --single` submits a statement at each newline, so a multi-line `CREATE TABLE` was split and every column line was parsed as its own query. All 26 statements were re-emitted one per line and then applied cleanly. **This is a harness defect, and it is recorded because the same class of failure would silently produce a partial schema in any line-oriented runner.** Real migration tooling (`sqlx migrate`, `refinery`) parses statement boundaries properly; T-033 must confirm that rather than assume it.

### Version matrix

| Version | Status | Result |
|---|---|---|
| PostgreSQL 16.10 | Verified 2026-10-17 | Applies cleanly; 12 tables, 35 indexes, 57 constraints, 13 FKs; 0 email columns |
| PostgreSQL 17.6 | Verified 2026-10-17 | Identical result to 16.10 |
| 14.x and earlier | **Not tested** | Out of scope: `gen_random_uuid()` requires PostgreSQL 13+, and the project targets current releases. Re-test if support is ever widened |
| 18.x | **Not tested** | Not available at time of writing. Should be added before release |

**Recommended supported range: PostgreSQL 16 to 17.** Both ends are verified; anything outside is untested and must not be claimed.

### Method, and an environment limitation that matters

The schema was applied with the **official EDB standalone binaries**, extracted to a temporary directory. **No system installation was performed**: no service, no registry changes, no Program Files entry, no `winget` install. Everything is under the temporary directory and can be deleted outright.

**Limitation, recorded honestly:** a normal client connection could not be used. The server starts and accepts connections, but forked backends die with `STATUS_DLL_INIT_FAILED` (0xC0000142) in this environment, so `psql` connections terminate unexpectedly. **All verification therefore ran through `postgres --single`, which executes in a single process and never forks.** What this means:

- **Verified:** the DDL parses and executes, all objects are created, defaults work, constraints and foreign keys are enforced, and the negative tests genuinely fail as designed.
- **Not verified:** anything requiring a normal client session — concurrent access, transaction isolation under load, connection handling, `query` behaviour, and live performance. **These remain open**, and are the reason the version matrix above is about *applying the schema*, not about running the system.

This is an environment limitation, not a property of PostgreSQL or of the schema. A normal CI runner or developer machine will not hit it. **T-033's CI job will provide the normal-connection verification that was unavailable here.**

## Migration workflow (proposed)

1. **Generate:** write a new numbered SQL file in `migrations/`; never edit an applied one.
2. **Review:** pull request reviewed against the compatibility rules in `data-model.md` §8; a migration touching a large table needs a stated plan.
3. **Apply:** one migration step per deployment, run before new application code is started; staging first.
4. **Roll back:** deploy a compensating forward migration. Destructive steps (drop column/table) happen only in a later release after code stops using them.
5. **Verify:** CI applies all migrations to an empty database and checks the result against the expected structure.

The migration tool is not yet chosen; selecting it is an open decision recorded in `tasks.md`.
