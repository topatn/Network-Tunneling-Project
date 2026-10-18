"""Generate the initial sqlx migration from schema-draft.sql.

Run once, by hand, on 2026-10-18. It exists so the migration is a byte-faithful copy of
the DDL that was already verified against PostgreSQL 16.10 and 17.6, rather than a
200-line transcription performed by an agent that could drop a constraint without noticing.

Idempotent: refuses to overwrite an existing migration.

    python scripts/gen_initial_migration.py
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "schema-draft.sql"
OUT = ROOT / "migrations"
STAMP = "20261018120000"
NAME = "initial_schema"

CREATE_TABLE = re.compile(r"^CREATE TABLE\s+([a-z_]+)\s*\(", re.I | re.M)


def main() -> int:
    if not SRC.is_file():
        print(f"error: {SRC} not found", file=sys.stderr)
        return 1
    OUT.mkdir(exist_ok=True)
    up = OUT / f"{STAMP}_{NAME}.up.sql"
    down = OUT / f"{STAMP}_{NAME}.down.sql"
    if up.exists() or down.exists():
        print("refusing to overwrite an existing migration", file=sys.stderr)
        return 1

    text = SRC.read_text(encoding="utf-8-sig")
    # Everything from the first CREATE TABLE onward is the schema body. The header
    # comment block records the authoring environment's verification history, which
    # belongs in schema.md rather than inside a migration.
    first = text.index("CREATE TABLE")
    body = text[first:].rstrip() + "\n"

    tables = CREATE_TABLE.findall(body)
    if not tables:
        print("error: no CREATE TABLE found", file=sys.stderr)
        return 1

    header_up = (
        f"-- {STAMP}_{NAME}\n"
        "-- Initial control-plane schema, generated from schema-draft.sql by\n"
        "-- scripts/gen_initial_migration.py. That file was applied with zero errors to an\n"
        "-- empty database on PostgreSQL 16.10 and 17.6, producing 12 tables, 35 indexes,\n"
        "-- 57 constraints and 13 foreign keys. See schema.md for the version matrix.\n"
        "--\n"
        "-- ROLLBACK is in the matching .down.sql. Per schema.md rule 4, a deployed system\n"
        "-- rolls back with a compensating forward migration, never by reverting this file.\n"
        f"-- Tables created here ({len(tables)}): {', '.join(tables)}\n\n"
    )

    # Reverse creation order. That is correct here because every foreign key in this
    # schema is declared inline at table creation, so a table's dependencies were all
    # created before it.
    header_down = (
        f"-- {STAMP}_{NAME}.down\n"
        "-- Reverses the initial schema, in reverse creation order. Every foreign key in\n"
        "-- this schema is declared inline at table creation, so reverse creation order is\n"
        "-- a valid drop order.\n"
        "--\n"
        "-- Per schema.md rule 4 this exists for local and test teardown. A deployed system\n"
        "-- rolls back with a compensating forward migration instead.\n\n"
    )
    drops = "".join(f"DROP TABLE IF EXISTS {t} CASCADE;\n" for t in reversed(tables))

    up.write_text(header_up + body, encoding="utf-8", newline="\n")
    down.write_text(header_down + drops, encoding="utf-8", newline="\n")

    print(f"wrote {up.relative_to(ROOT)}  ({len(body.splitlines())} body lines)")
    print(f"wrote {down.relative_to(ROOT)} ({len(tables)} DROP statements)")
    print(f"tables: {len(tables)} -> {', '.join(tables)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
