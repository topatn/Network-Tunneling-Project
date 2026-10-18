"""Apply a migration to a throwaway PostgreSQL cluster via `postgres --single`.

Exists because this machine cannot fork a PostgreSQL backend (STATUS_DLL_INIT_FAILED),
so the only local way to execute DDL is `--single`, which does not fork.

THE TRAP, hit twice on 2026-10-18 and recorded in schema.md: `postgres --single` submits
ONE STATEMENT PER LINE. Feeding it multi-line DDL shreds every CREATE TABLE into a syntax
error and produces a zero-table schema that looks like success in some harnesses. This
script collapses each statement onto one line before submission.

It is a measurement aid, not a migration runner. `sqlx migrate` in CI is the real tool
(ADR-017); this exists only to obtain measured counts without waiting for CI.

    python scripts/apply_via_single.py <cluster-data-dir> <migration.up.sql>
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path


def strip_comments(sql: str) -> str:
    out, i, n = [], 0, len(sql)
    in_s = in_d = False
    while i < n:
        c = sql[i]
        if c == "'" and not in_d:
            in_s = not in_s
        elif c == '"' and not in_s:
            in_d = not in_d
        elif c == "-" and not in_s and not in_d and sql[i:i + 2] == "--":
            while i < n and sql[i] != "\n":
                i += 1
            continue
        out.append(c)
        i += 1
    return "".join(out)


def collapse(sql: str) -> str:
    """One statement per line: strip comments, then flatten all internal whitespace."""
    body = strip_comments(sql)
    statements = []
    buf = []
    in_s = False
    i = 0
    while i < len(body):
        c = body[i]
        if c == "'":
            in_s = not in_s
        if c == ";" and not in_s:
            s = re.sub(r"\s+", " ", "".join(buf)).strip()
            if s:
                statements.append(s)
            buf = []
        else:
            buf.append(c)
        i += 1
    tail = re.sub(r"\s+", " ", "".join(buf)).strip()
    if tail:
        statements.append(tail)
    return "\n".join(statements) + "\n"


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    data_dir = Path(sys.argv[1])
    migration = Path(sys.argv[2])
    pg_bin = Path(sys.argv[0]).resolve().parent  # unused; caller sets PATH

    sql = migration.read_text(encoding="utf-8")
    script = collapse(sql)
    lines = [ln for ln in script.splitlines() if ln.strip()]
    print(f"collapsed {len(sql.splitlines())} source lines into {len(lines)} statements")

    # postgres.exe is resolved from PATH by the caller.
    proc = subprocess.run(
        ["postgres", "--single", "-D", str(data_dir), "postgres"],
        input=script, capture_output=True, text=True, timeout=300,
    )
    err = proc.stderr or ""
    errors = [ln for ln in err.splitlines() if "ERROR" in ln or "FATAL" in ln]
    if errors:
        print(f"FAILED with {len(errors)} error line(s):", file=sys.stderr)
        for ln in errors[:12]:
            print("  " + ln, file=sys.stderr)
        return 1
    print("no ERROR/FATAL in server output")
    return 0


if __name__ == "__main__":
    sys.exit(main())
