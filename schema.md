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

- `schema-draft.sql` parses with the PostgreSQL parser (12 tables, 14 indexes). Syntax only; **not executed** against a database, so defaults, extension availability (`gen_random_uuid()`), and constraint interplay are unverified. A task in `tasks.md` must run it on a real PostgreSQL version and record which versions are supported.
- Invariants INV-2, INV-4, INV-5 are application-level and need tests (`data-model.md` §6).

## Migration workflow (proposed)

1. **Generate:** write a new numbered SQL file in `migrations/`; never edit an applied one.
2. **Review:** pull request reviewed against the compatibility rules in `data-model.md` §8; a migration touching a large table needs a stated plan.
3. **Apply:** one migration step per deployment, run before new application code is started; staging first.
4. **Roll back:** deploy a compensating forward migration. Destructive steps (drop column/table) happen only in a later release after code stops using them.
5. **Verify:** CI applies all migrations to an empty database and checks the result against the expected structure.

The migration tool is not yet chosen; selecting it is an open decision recorded in `tasks.md`.
