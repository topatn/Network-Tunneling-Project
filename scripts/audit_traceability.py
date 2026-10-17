#!/usr/bin/env python3
"""Cross-document traceability audit.

Checks that the planning document set is internally consistent in the ways the
project depends on. This exists because the counts in requirements.md, tasks.md
and plan.md are hand-maintained, and T-002 added eleven requirements (SRV-001 to
SRV-011) whose mappings were written by hand rather than generated.

It is a *consistency* checker, not a correctness checker: passing means the
documents agree with each other, not that the design is right.

    python scripts/audit_traceability.py            # report
    python scripts/audit_traceability.py --strict   # warnings are failures too

Exit codes: 0 = no errors, 1 = errors found.

Self-test: `--self-test` copies the repository's Markdown into a temporary tree,
plants four defects, and asserts each is detected. An audit script that cannot be
shown to fail is not evidence of anything -- that is the whole point of the
seeded-vulnerability check recorded in docs/dependency-register.md.
"""

from __future__ import annotations

import argparse
import re
import shutil
import sys
import tempfile
from collections.abc import Iterator
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Every pattern used with finditer()/search() over whole-file text needs
# MULTILINE. Getting this wrong makes the audit silently match nothing, which
# looks identical to a clean pass.
M = re.MULTILINE

REQ_LINE = re.compile(r"^\|\s*\*\*([A-Z]+-\d+)\*\*\s*\|", M)
APPENDIX_ROW = re.compile(r"^\|\s*([A-Z]+-\d+)\s*\|[^|]*\|\s*([^|]+?)\s*\|", M)
TASK_HEADING = re.compile(r"^###\s+(T-\d+)\b", M)
TASK_PREREQ = re.compile(r"^- \[.\]\s+\*\*Prerequisites:\*\*\s*(.*)$", M)
TASK_REQ_FIELD = re.compile(r"^- \*\*Requirements:\*\*\s*(.*)$", M)
EXP_ROW = re.compile(r"^\|\s*(E-\d+)\s*\|", M)
ADR_ROW = re.compile(r"^\|\s*(ADR-\d+)\s*\|", M)
ADR_HEADING = re.compile(r"^###\s+(ADR-\d+)\b", M)
DM_TABLE_HEADING = re.compile(r"^###\s+([a-z_]+)\s*$", M)
DM_FIELDS = re.compile(r"^- \*\*Fields:\*\*\s*(.*)$", M)
DDL_TABLE = re.compile(r"CREATE TABLE\s+([a-z_]+)\s*\(", re.I)
CT_ROUTE_SECTION = re.compile(
    r"^###\s+(\d+(?:\.\d+)?)\s+(.*?)$(?P<body>.*?)(?=^###\s|^##\s|\Z)", M | re.S
)
CT_SECTION = re.compile(r"^##\s+(\d+)\.\s+(.*?)$(?P<body>.*?)(?=^##\s|\Z)", M | re.S)
ROUTE_TOKEN = re.compile(r"`([A-Z]+)\s+(/[^`]+)`")
ROUTE_LIST_ITEM = re.compile(r"^-\s+`([A-Z]+)\s+(/[^`]+)`", M)
API_ROW = re.compile(r"^\|\s*(\d+)\s*\|")
COUNTS_LINE = re.compile(r"(\d+)\s+requirements,\s*(\d+)\s+tasks")
PLAN_TOTALS = re.compile(r"Total:\s*(\d+)\s+tasks,\s*(\d+)\s+requirements")


def read(root: Path, name: str) -> str:
    # utf-8-sig tolerates a BOM, which some PowerShell writers emit.
    return (root / name).read_text(encoding="utf-8-sig")


def _clean_routes(pairs: list[tuple[str, str]]) -> set[tuple[str, str]]:
    """Normalise route tokens: strip backticks, query strings, and whitespace.

    The query-string strip is not cosmetic. Section 7 writes the route as
    "`GET /internal/v1/relays/{relay_id}/state?cursor=<opaque>`", so a pattern that
    required the closing backtick immediately after the path matched nothing and
    contracts.md section 7 appeared to define only one of its two routes.
    """
    out = set()
    for meth, path in pairs:
        out.add((meth, path.split("?")[0].split("#")[0].strip()))
    return out


def contract_routes(contracts_text: str) -> dict[str, dict]:
    """Route definitions from contracts.md, keyed by section number.

    Three shapes have to be handled, and getting this wrong is how the first attempt
    reported four phantom errors:

    - Numbered subsections: '### 5.7 `GET /v1/tunnels` and `GET /v1/tunnels/{id}`' --
      several routes in one heading.
    - Prose sections whose routes are unnumbered sub-headings: section 6 is
      '### `POST /v1/invitations/claim` **[E-13]**'.
    - Prose sections whose routes are list items: section 7 is a pair of
      '- `GET /internal/...`' bullets.

    Only the section's *own* content counts. A parent section such as '## 5. User HTTP
    API' must not absorb its subsections' routes, and a prose section that merely
    mentions a route in passing -- section 4 names 'POST /v1/invitations/claim' while
    describing rate-limit classes, section 8 names 'POST /v1/tunnels' while describing
    idempotency -- defines nothing. Requiring an unnumbered sub-heading or a leading
    '- `METHOD path`' list item is what separates a definition from a mention.
    """
    out: dict[str, dict] = {}
    for m in CT_ROUTE_SECTION.finditer(contracts_text):
        own = ROUTE_TOKEN.findall(m.group(2))
        own.extend(ROUTE_LIST_ITEM.findall(m.group("body")))
        out[m.group(1)] = {
            "routes": _clean_routes(own),
            "success": re.findall(r"[Ss]uccess\s+`?(\d{3})`?", m.group("body")),
            "title": m.group(2).strip(),
        }

    for m in CT_SECTION.finditer(contracts_text):
        num = m.group(1)
        body = m.group("body")
        own: list[tuple[str, str]] = []
        for part in re.split(r"^###\s", body, flags=M)[1:]:
            if re.match(r"\d", part):
                continue  # numbered subsection, indexed above
            own.extend(ROUTE_TOKEN.findall(part.splitlines()[0]))
        own.extend(ROUTE_LIST_ITEM.findall(body))
        if not own:
            continue
        out[num] = {
            "routes": _clean_routes(own),
            "success": re.findall(r"[Ss]uccess\s+`?(\d{3})`?", body),
            "title": m.group(2).strip(),
        }
    return out


def api_rows(api_text: str) -> list[dict]:
    """Route rows from api.md, with column meaning taken from each table's header.

    The three tables have different column sets -- the user table has an Idem column,
    the guest table does not, and the internal table has no Success column at all -- so
    positions are read from the header rather than hard-coded. Hard-coding them is how the
    first attempt failed, with a ValueError on unpack.
    """
    rows: list[dict] = []
    header: list[str] | None = None
    for line in api_text.splitlines():
        s = line.strip()
        if not s.startswith("|"):
            header = None
            continue
        cells = [c.strip() for c in s.strip("|").split("|")]
        if cells and cells[0] == "#":
            header = [c.lower() for c in cells]
            continue
        if not (cells and cells[0].isdigit()):
            continue
        if not header:
            continue
        rec = dict(zip(header, cells))
        rec["num"] = cells[0]
        rows.append(rec)
    return rows


def strip_sql_comments(sql_text: str) -> str:
    """Remove `--` line comments, honouring quoted strings.

    guest_admissions has `source_ip inet NOT NULL,   -- sensitive; short retention`.
    Splitting on comma-newline before stripping comments glued that whole line to the
    next one, so `created_at` vanished from the parsed columns and the check reported a
    missing column in a table that has it. Comment stripping must happen first.
    """
    out_lines: list[str] = []
    for line in sql_text.splitlines():
        in_single = in_double = False
        cut = len(line)
        j = 0
        while j < len(line):
            ch = line[j]
            if ch == "'" and not in_double:
                in_single = not in_single
            elif ch == '"' and not in_single:
                in_double = not in_double
            elif ch == "-" and not in_single and not in_double:
                if j + 1 < len(line) and line[j + 1] == "-":
                    cut = j
                    break
            j += 1
        out_lines.append(line[:cut])
    return "\n".join(out_lines)


def ddl_columns(sql_text: str) -> dict[str, list[str]]:
    """Column names per CREATE TABLE, from the DDL text.

    Statement bodies are located by paren-depth walking rather than by splitting on
    commas, because a column definition may itself contain commas (CHECK with an
    IN list, a numeric type). A naive comma split produced phantom columns named
    "AND" and duplicate column names on the first attempt at this.
    """
    out: dict[str, list[str]] = {}
    for m in DDL_TABLE.finditer(sql_text):
        table = m.group(1)
        depth, i = 1, m.end()
        while i < len(sql_text) and depth:
            if sql_text[i] == "(":
                depth += 1
            elif sql_text[i] == ")":
                depth -= 1
            i += 1
        body = strip_sql_comments(sql_text[m.end(): i - 1])
        cols: list[str] = []
        for raw in re.split(r",\s*\n", body):
            item = raw.strip().rstrip(",").strip()
            if not item:
                continue
            if re.match(r"^(CONSTRAINT|PRIMARY KEY|FOREIGN KEY|UNIQUE|CHECK|EXCLUDE)\b",
                        item, re.I):
                continue
            cm = re.match(r"^([a-z_][a-z0-9_]*)\s+\S", item, re.I)
            if cm:
                cols.append(cm.group(1))
        out[table] = cols
    return out


def documented_columns(model_text: str) -> dict[str, list[str]]:
    """Column names per entity, from data-model.md's '**Fields:**' lines.

    Two exclusions, both learned from writing this the obvious way first and being
    wrong:

    - Tokens inside parentheses are dropped. guest_admissions says
      "(composite FK to `invitations(id, tunnel_id)`)" -- that is a *reference* to a
      column on another table, not a column of this one. Including it reported
      guest_admissions as having an 'invitations' column.
    - Only backticked names at paren depth 0 are collected, and every one of them is
      kept rather than just the first per semicolon-separated segment. Two real
      formatting styles exist in this document: semicolon-separated groups
      ("`a`; `b`") and comma-separated ones ("`a FK CASCADE`, `b text`, `c`").
      Taking only the first token per segment silently dropped seven columns across
      four tables and produced six false 'missing column' errors on a clean repo.

    Fields lines that use neither style consistently are a formatting question for
    data-model.md, not something to paper over here: whatever this function cannot
    parse shows up as an error naming the table, so a formatting change cannot pass
    unnoticed.
    """
    out: dict[str, list[str]] = {}
    for m in DM_TABLE_HEADING.finditer(model_text):
        table = m.group(1)
        fm = DM_FIELDS.search(model_text[m.end():])
        if not fm:
            continue
        line = fm.group(1)
        cols: list[str] = []
        depth = 0
        i = 0
        while i < len(line):
            ch = line[i]
            if ch == "(":
                depth += 1
            elif ch == ")":
                depth = max(0, depth - 1)
            elif ch == "`" and depth == 0:
                end = line.find("`", i + 1)
                if end != -1:
                    token = line[i + 1:end]
                    name = re.match(r"^([a-z_][a-z0-9_]*)", token)
                    if name:
                        cols.append(name.group(1))
                    i = end
            i += 1
        out[table] = cols
    return out


def audit(root: Path) -> tuple[list[str], list[str]]:
    errors: list[str] = []
    warnings: list[str] = []

    req_text = read(root, "requirements.md")
    task_text = read(root, "tasks.md")
    plan_text = read(root, "plan.md")
    research_text = read(root, "research.md")
    model_text = read(root, "data-model.md")
    contracts_text = read(root, "contracts.md")
    arch_text = read(root, "architecture.md")

    declared = {m.group(1) for m in REQ_LINE.finditer(req_text)}
    appendix = {m.group(1) for m in APPENDIX_ROW.finditer(req_text)}
    tasks = {m.group(1) for m in TASK_HEADING.finditer(task_text)}
    experiments = {m.group(1) for m in EXP_ROW.finditer(research_text)}
    adrs = {m.group(1) for m in ADR_ROW.finditer(research_text)}
    adr_sections = {m.group(1) for m in ADR_HEADING.finditer(research_text)}
    invariants = set(re.findall(r"(INV-\d+)", model_text))

    if not declared:
        errors.append(
            "no requirements parsed from requirements.md -- the audit is broken, "
            "not the documents"
        )
    if not tasks:
        errors.append(
            "no tasks parsed from tasks.md -- the audit is broken, not the documents"
        )

    # 1. Every declared requirement has a traceability row.
    for rid in sorted(declared - appendix):
        errors.append(f"{rid} declared in requirements.md but has no traceability row")

    # 2. Every traceability row refers to a real requirement.
    for rid in sorted(appendix - declared):
        errors.append(f"{rid} in the traceability appendix but is not declared")

    # 3. Every traceability row names at least one task, and they exist.
    for rid, field in APPENDIX_ROW.findall(req_text):
        if rid not in declared:
            continue
        named = re.findall(r"T-\d+", field)
        if not named:
            errors.append(f"{rid} maps to no task")
        for t in named:
            if t not in tasks:
                errors.append(f"{rid} maps to unknown task {t}")

    # Split tasks into blocks so each is checked on its own.
    blocks: dict[str, str] = {}
    current = None
    buf: list[str] = []
    for line in task_text.splitlines():
        m = TASK_HEADING.match(line)
        if m:
            if current:
                blocks[current] = "\n".join(buf)
            current, buf = m.group(1), []
        elif current:
            buf.append(line)
    if current:
        blocks[current] = "\n".join(buf)

    # 4. Every task cites at least one real requirement.
    for tid, block in sorted(blocks.items()):
        field = TASK_REQ_FIELD.search(block)
        if not field:
            errors.append(f"{tid} has no Requirements field")
            continue
        named = re.findall(r"[A-Z]+-\d+", field.group(1))
        if not named:
            errors.append(f"{tid} cites no requirement")
        for rid in named:
            if rid not in declared:
                errors.append(f"{tid} cites unknown requirement {rid}")

    # 5. Prerequisites exist; detect true cycles order-independently; report
    #    forward numeric references as warnings only.
    #
    #    A forward reference (T-043 depends on T-050) is NOT a defect: task IDs
    #    are stable identifiers and prerequisites are inserted ahead of the gate
    #    they feed. A genuine cycle (A -> B -> A) is fatal because no
    #    implementation order exists. Conflating the two once made this audit
    #    report seven errors when only the cycles were real.
    graph: dict[str, list[str]] = {}
    for tid, block in sorted(blocks.items()):
        m = TASK_PREREQ.search(block)
        if not m:
            errors.append(f"{tid} has no Prerequisites field")
            graph[tid] = []
            continue
        deps: list[str] = []
        for p in re.findall(r"T-\d+", m.group(1)):
            if p not in tasks:
                errors.append(f"{tid} prerequisite {p} does not exist")
            else:
                deps.append(p)
                if int(p[2:]) >= int(tid[2:]):
                    warnings.append(
                        f"{tid} depends on the later-numbered {p} "
                        f"(allowed; IDs are stable, Prerequisites is authoritative)"
                    )
        graph[tid] = deps

    for cycle in find_cycles(graph):
        errors.append("prerequisite cycle: " + " -> ".join(cycle))

    # 6. Cross-document references resolve.
    haystack = req_text + contracts_text + task_text + arch_text
    for label, pattern, known in (
        ("experiment", re.compile(r"\bE-\d+\b"), experiments),
        ("ADR", re.compile(r"\bADR-\d+\b"), adrs),
        ("invariant", re.compile(r"\bINV-\d+\b"), invariants),
    ):
        for ref in sorted(set(pattern.findall(haystack))):
            if ref not in known:
                errors.append(f"{label} {ref} referenced but never defined")

    # 6b. The ADR summary table and the ADR full records must agree.
    #
    #     Added 2026-10-17 after ADR-016 was found to have a complete record in
    #     section 7 but no row in the summary table directly above it, while
    #     ADR-017 had both. A reader scanning the table would conclude the
    #     decision had not been made. Check 6 only proved that a *referenced*
    #     ADR number resolves to *some* definition; it cannot see an ADR that is
    #     defined but unrepresented.
    for adr in sorted(adr_sections - adrs):
        errors.append(
            f"{adr} has a full record in research.md section 7 but no row in "
            f"the ADR summary table"
        )
    for adr in sorted(adrs - adr_sections):
        warnings.append(
            f"{adr} is listed in the ADR summary table but has no full record "
            f"in research.md section 7"
        )

    # 7. Cited files and paths must actually exist.
    #
    #    Added 2026-10-17 after finding that contracts.md, api.md,
    #    data-model.md and tasks.md all cited docs/contracts/ and
    #    contracts/examples.json as authoritative while neither existed. Every
    #    earlier check passed cleanly on that state, because they only validated
    #    identifiers. A document set can be perfectly self-consistent and still
    #    point at files that were never written.
    path_refs: set[str] = set()
    for text in (req_text, task_text, plan_text, research_text, model_text,
                 contracts_text, arch_text):
        for m in re.finditer(
            r"`((?:docs|contracts|migrations|profiles|crates|tests|infra|apps|scripts)"
            r"(?:/[A-Za-z0-9._-]+)+)`",
            text,
        ):
            path_refs.add(m.group(1))

    # Not-yet-written files are legitimate for a planning-phase repository: a
    # task may name the artifact it will produce. These are expected absences,
    # recorded so the gate stays green while the fact stays visible. Removing an
    # entry from this set is a deliberate act that should be reviewed.
    EXPECTED_ABSENT = {
        # To be authored by T-031 (protocol crate: schemas and examples).
        "contracts/examples.json",
        "contracts/error.schema.json",
        "contracts/tunnel-hello.schema.json",
        "contracts/tunnel-hello-ack.schema.json",
        "contracts/create-tunnel-request.schema.json",
        "contracts/start-tunnel-response.schema.json",
        "contracts/create-invitation-request.schema.json",
        "contracts/create-invitation-response.schema.json",
        "contracts/claim-invitation-request.schema.json",
        "contracts/claim-invitation-response.schema.json",
        "contracts/tunnel.schema.json",
        "contracts/error-response.schema.json",
    }

    missing_paths = sorted(p for p in path_refs if not (root / p).exists())
    for p in missing_paths:
        if p in EXPECTED_ABSENT:
            warnings.append(f"cited path not written yet (expected): {p}")
        else:
            errors.append(f"cited path does not exist: {p}")

    # 9. data-model.md and the DDL must describe the same tables and columns.
    #
    #    Added 2026-10-17. T-120's "does not cover API/data-model agreement" was a
    #    standing gap: data-model.md is authoritative for entities and
    #    schema-draft.sql for physical shape, and nothing compared them. A column
    #    could be added to one and not the other indefinitely and every other check
    #    would still pass, because identifiers would all still resolve.
    ddl_file = root / "schema-draft.sql"
    if not ddl_file.exists():
        warnings.append("schema-draft.sql not found; skipping model/DDL agreement check")
    else:
        ddl = ddl_columns(ddl_file.read_text(encoding="utf-8-sig"))
        doc = documented_columns(model_text)
        if not ddl or not doc:
            errors.append(
                "model/DDL agreement check parsed nothing "
                f"(DDL tables={len(ddl)}, documented={len(doc)}) -- the check is broken"
            )
        for table in sorted(set(ddl) - set(doc)):
            errors.append(
                f"table {table} exists in the DDL but is not documented in data-model.md"
            )
        for table in sorted(set(doc) - set(ddl)):
            errors.append(
                f"table {table} is documented in data-model.md but absent from the DDL"
            )
        for table in sorted(set(ddl) & set(doc)):
            only_ddl = [c for c in ddl[table] if c not in doc[table]]
            only_doc = [c for c in doc[table] if c not in ddl[table]]
            if only_ddl:
                errors.append(
                    f"{table}: column(s) {', '.join(only_ddl)} are in the DDL but not "
                    f"in data-model.md"
                )
            if only_doc:
                errors.append(
                    f"{table}: column(s) {', '.join(only_doc)} are in data-model.md but "
                    f"not in the DDL"
                )

    # 10. api.md and contracts.md must agree on every route.
    #
    #     Added 2026-10-17, closing the remaining "API/data-model agreement" gap that
    #     T-120 and requirements.md both named as open. api.md is an inventory and
    #     contracts.md is authoritative for wire format; nothing compared them, so a
    #     route could be renamed, moved, or given a different status code in one file
    #     and every other check would still pass.
    api_file = root / "api.md"
    if not api_file.exists():
        errors.append("api.md is missing; the route agreement check cannot run")
    else:
        api_text = read(root, "api.md")
        sections = contract_routes(contracts_text)
        rows = api_rows(api_text)
        if not rows or not sections:
            errors.append(
                f"route agreement check parsed nothing (api rows={len(rows)}, "
                f"contract sections={len(sections)}) -- the check is broken"
            )
        covered: set[str] = set()
        for rec in rows:
            method = rec.get("method", "")
            path = rec.get("path", "").strip("`")
            ref = rec.get("contract", "").strip().lstrip("\u00a7")
            key = f"{method} {path}"
            if ref not in sections:
                errors.append(
                    f"api.md route {key} cites contracts.md section {ref or '(none)'}, "
                    f"which does not exist"
                )
                continue
            covered.add(ref)
            section = sections[ref]
            if (method, path) not in section["routes"]:
                errors.append(
                    f"api.md route {key} is not defined in contracts.md section {ref} "
                    f"(that section defines: "
                    f"{', '.join(sorted(m + ' ' + p for m, p in section['routes'])) or 'nothing'})"
                )
                continue
            stated = rec.get("success", "")
            api_codes = re.findall(r"\b(\d{3})\b", stated)
            if api_codes and not api_codes[0].startswith("2"):
                errors.append(
                    f"api.md lists success {api_codes[0]} for {key}; a Success column "
                    f"must hold a 2xx status"
                )
            elif api_codes and not section["success"]:
                errors.append(
                    f"api.md gives success {api_codes[0]} for {key}, but contracts.md "
                    f"section {ref} -- the authoritative document -- states no success "
                    f"code at all"
                )
            elif api_codes and not set(api_codes) & set(section["success"]):
                errors.append(
                    f"api.md gives success {api_codes[0]} for {key}, but contracts.md "
                    f"section {ref} states {sorted(set(section['success']))}"
                )
        # A route defined in contracts.md but absent from the inventory is the same
        # drift in the other direction: the contract promises an endpoint nothing lists.
        for ref in sorted(sections, key=lambda s: [int(p) for p in s.split(".")]):
            if ref not in covered and sections[ref]["routes"]:
                errors.append(
                    f"contracts.md section {ref} defines "
                    f"{', '.join(sorted(m + ' ' + p for m, p in sections[ref]['routes']))} "
                    f"but no api.md row cites it"
                )

    # 11. Workflows must agree with the tools they invoke.
    #
    #     Added 2026-10-17 after writing .github/workflows/clean-vm.yml and finding two
    #     defects in it by reading rather than running: artifact paths used backslashes,
    #     which upload-artifact's glob matcher does not treat as a separator, so the run
    #     would have silently produced no artifact; and the phase-1 step conflated
    #     "a check failed" with "wrong exit code", which would have misattributed a real
    #     failure. Neither could run, so nothing else would have caught either.
    workflow_dir = root / ".github" / "workflows"
    verify_src = ""
    verify_file = root / "scripts" / "verify_clean_vm.py"
    if verify_file.exists():
        verify_src = verify_file.read_text(encoding="utf-8-sig")

    exit_codes: dict[str, str] = {}
    # The constants are a tuple assignment, not one per line. An earlier version of this
    # check used a per-line pattern, matched nothing, and reported agreement it had not
    # actually verified.
    tuple_assign = re.search(
        r"^(EXIT_\w+(?:\s*,\s*EXIT_\w+)*)\s*=\s*([\d,\s]+)$", verify_src, re.M
    )
    if tuple_assign:
        names = [n.strip() for n in tuple_assign.group(1).split(",")]
        values = [v.strip() for v in tuple_assign.group(2).split(",")]
        if len(names) == len(values):
            exit_codes = dict(zip(names, values))

    if workflow_dir.is_dir():
        for wf in sorted(workflow_dir.glob("*.yml")):
            text = read(root, f".github/workflows/{wf.name}")
            # Artifact paths live inside a YAML literal block (`path: |`), so each entry
            # is a bare indented line, NOT a "- " list item. The first version of this
            # check matched a list prefix, never matched anything, and sat there passing
            # silently -- a false pass in the checker written to catch a false pass.
            for block in re.finditer(
                r"^\s*path:\s*\|\s*\n((?:[ \t]+[^\n]*\n)+)", text, re.M
            ):
                for line in block.group(1).splitlines():
                    entry = line.strip()
                    if not entry or entry.startswith("#"):
                        continue
                    if "\\" in entry:
                        errors.append(
                            f"{wf.name}: artifact path {entry!r} uses a backslash. "
                            f"upload-artifact takes glob patterns, which do not treat a "
                            f"backslash as a separator, so this silently matches nothing"
                        )
            for cm in re.finditer(r"LASTEXITCODE -ne (\d+)", text):
                asserted = cm.group(1)
                expected = exit_codes.get("EXIT_OPERATOR")
                if expected is None:
                    warnings.append(
                        f"{wf.name}: asserts exit {asserted} but EXIT_OPERATOR could "
                        f"not be read from scripts/verify_clean_vm.py"
                    )
                elif asserted != expected:
                    errors.append(
                        f"{wf.name}: asserts exit {asserted} where "
                        f"scripts/verify_clean_vm.py defines EXIT_OPERATOR={expected}"
                    )

            # $GITHUB_ENV is parsed by GitHub, not by a shell, and it does not tolerate a
            # byte-order mark. Every PowerShell cmdlet here emits one by default, and a
            # BOM is invisible in an editor, so the mistake survives every local read and
            # only appears as "Invalid format 'image: win22'" on a hosted runner.
            # Proven the hard way: clean-vm.yml failed its first real run on exactly this.
            for wm in re.finditer(
                r"(Out-File|Set-Content|Add-Content)\b[^\n]*GITHUB_ENV", text
            ):
                errors.append(
                    f"{wf.name}: writes to $GITHUB_ENV with {wm.group(1)}, which emits a "
                    f"UTF-8 BOM that GitHub's env-file parser rejects. Use "
                    f"[System.IO.File]::AppendAllText with an explicit "
                    f"UTF8Encoding($false), or write to the log instead"
                )
            for rm in re.finditer(r">>\s*\"?\$\{?\{?\s*env:GITHUB_ENV", text):
                errors.append(
                    f"{wf.name}: appends to $GITHUB_ENV with shell redirection, whose "
                    f"encoding depends on the shell. Use an explicit BOM-free write"
                )

    # 8. Declared counts must match what is actually there.
    m = COUNTS_LINE.search(req_text)
    if m:
        if int(m.group(1)) != len(declared):
            errors.append(
                f"requirements.md claims {m.group(1)} requirements, found {len(declared)}"
            )
        if int(m.group(2)) != len(tasks):
            errors.append(f"requirements.md claims {m.group(2)} tasks, found {len(tasks)}")
    else:
        warnings.append("requirements.md has no 'N requirements, N tasks' line")

    m = PLAN_TOTALS.search(plan_text)
    if m:
        if int(m.group(1)) != len(tasks):
            errors.append(f"plan.md claims {m.group(1)} tasks, found {len(tasks)}")
        if int(m.group(2)) != len(declared):
            errors.append(
                f"plan.md claims {m.group(2)} requirements, found {len(declared)}"
            )
    else:
        warnings.append("plan.md has no 'Total: N tasks, N requirements' line")

    chain = longest_chain(graph)
    stats = {
        "requirements declared": len(declared),
        "traceability rows": len(appendix),
        "tasks": len(tasks),
        "experiments": len(experiments),
        "ADRs (table rows)": len(adrs),
        "ADRs (full records)": len(adr_sections),
        "invariants": len(invariants),
        "model/DDL tables compared": len(ddl) if ddl_file.exists() else 0,
        "longest chain (tasks)": len(chain),
    }
    chain_text = " -> ".join(chain)
    errors, warnings, stats = errors, warnings, stats
    return errors, warnings, stats, chain_text


def longest_chain(graph: dict[str, list[str]]) -> list[str]:
    """Longest prerequisite chain, counted in tasks.

    Replaces the hand-derived critical path. Depth is computed with an
    order-independent DFS, so a task depending on a higher-numbered task is
    handled correctly. Ties are broken deterministically (alphabetically) so the
    output is stable between runs.
    """
    memo: dict[str, int] = {}

    def depth(node: str, seen: frozenset[str] = frozenset()) -> int:
        if node in memo:
            return memo[node]
        if node in seen:  # defensive; find_cycles reports real cycles
            return 0
        best = 1
        for dep in graph.get(node, []):
            if dep in graph:
                best = max(best, 1 + depth(dep, seen | {node}))
        memo[node] = best
        return best

    best_node = ""
    best_depth = 0
    for node in sorted(graph):
        d = depth(node)
        if d > best_depth:
            best_node, best_depth = node, d
    if not best_node:
        return []

    chain: list[str] = []
    node = best_node
    while True:
        chain.append(node)
        nxt = [d for d in graph.get(node, []) if d in graph and depth(d) == depth(node) - 1]
        if not nxt:
            return list(reversed(chain))
        node = sorted(nxt)[0]


def find_cycles(graph: dict[str, list[str]]) -> list[list[str]]:
    """Return every elementary prerequisite cycle, detected without relying on
    numeric ordering.

    Frame-based DFS so that `path` is always exactly the current recursion
    stack. An earlier version tracked the path per stack entry, which desynchronises
    on diamond-shaped graphs and crashes with 'not in list'.
    """
    WHITE, GREY, BLACK = 0, 1, 2
    colour = {node: WHITE for node in graph}
    cycles: list[list[str]] = []
    seen: set[tuple[str, ...]] = set()

    for start in sorted(graph):
        if colour[start] != WHITE:
            continue
        colour[start] = GREY
        path = [start]
        stack: list[Iterator[str]] = [iter(graph.get(start, []))]

        while stack:
            advanced = False
            for dep in stack[-1]:
                if dep not in colour:
                    continue
                if colour[dep] == GREY:
                    cycle = path[path.index(dep):] + [dep]
                    key = tuple(sorted(set(cycle)))
                    if key not in seen:
                        seen.add(key)
                        cycles.append(cycle)
                elif colour[dep] == WHITE:
                    colour[dep] = GREY
                    path.append(dep)
                    stack.append(iter(graph.get(dep, [])))
                    advanced = True
                    break
            if not advanced:
                colour[path.pop()] = BLACK
                stack.pop()

    return cycles


def self_test(root: Path) -> int:
    """Prove the audit detects defects. A checker never seen to fail proves nothing."""
    defects = [
        (
            "unmapped requirement",
            "requirements.md",
            lambda t: t + "\n| **FR-999** | Invented. | P0 | x | x | Unit test | - | 1.0 |\n",
            "FR-999",
        ),
        (
            "task citing a nonexistent requirement",
            "tasks.md",
            lambda t: t + "\n### T-999 — Invented\n\n- [ ] **Prerequisites:** none\n"
            "- **Requirements:** FR-999\n",
            "T-999 cites unknown requirement",
        ),
        (
            "true prerequisite cycle (T-001 -> T-002 -> T-001)",
            "tasks.md",
            lambda t: t.replace(
                "### T-002 — Replace reported game facts with primary sources [CP]\n\n"
                "- [ ] **Prerequisites:** T-001",
                "### T-002 — Replace reported game facts with primary sources [CP]\n\n"
                "- [ ] **Prerequisites:** T-001, T-001",
            ).replace(
                "### T-001 — Create repository and CI skeleton [CP]\n\n- [ ] **Prerequisites:** none",
                "### T-001 — Create repository and CI skeleton [CP]\n\n- [ ] **Prerequisites:** T-002",
            ),
            "prerequisite cycle",
        ),
        (
            "forward reference to a later task (must be a WARNING, not an error)",
            "tasks.md",
            lambda t: t.replace(
                "### T-001 — Create repository and CI skeleton [CP]\n\n- [ ] **Prerequisites:** none",
                "### T-001 — Create repository and CI skeleton [CP]\n\n- [ ] **Prerequisites:** T-004",
            ),
            "allowed; IDs are stable",
        ),
        (
            "cited file that does not exist",
            "contracts.md",
            lambda t: t.replace(
                "`contracts.md`", "`contracts/does-not-exist.schema.json`", 1
            ),
            "cited path does not exist",
        ),
        (
            "wrong declared count",
            "requirements.md",
            lambda t: re.sub(r"Counts: \d+ requirements", "Counts: 999 requirements", t, count=1),
            "requirements, found",
        ),
        (
            "ADR with a full record but no summary-table row",
            "research.md",
            lambda t: re.sub(
                r"^\| ADR-016 \|[^\n]*\n", "", t, count=1, flags=M
            ),
            "no row in the ADR summary table",
        ),
        (
            "summary-table row for an ADR with no full record",
            "research.md",
            lambda t: t + "\n| ADR-099 | Invented decision | Proposed | 2026-10-17 | Low |\n",
            "no full record",
        ),
        (
            "column in data-model.md that the DDL does not have",
            "data-model.md",
            lambda t: t.replace(
                "### audit_events\n- **Fields:** `id bigint identity PK`;",
                "### audit_events\n- **Fields:** `id bigint identity PK`; "
                "`invented_column timestamptz`;",
                1,
            ),
            "are in data-model.md but not in the DDL",
        ),
        (
            "table in the DDL that data-model.md does not document",
            "data-model.md",
            lambda t: t + "\n### shadow_table\n- **Fields:** `id uuid PK`.\n",
            "is documented in data-model.md but absent from the DDL",
        ),
        (
            "route in api.md that contracts.md does not define at that path",
            "api.md",
            lambda t: t.replace(
                "| 4 | GET | `/v1/me` |", "| 4 | GET | `/v1/mystery` |", 1
            ),
            "is not defined in contracts.md section 5.4",
        ),
        (
            "success status in api.md that the authoritative contracts.md contradicts",
            "api.md",
            # Matched without the section symbol: the planted text must not depend on a
            # non-ASCII character surviving the write, or the defect silently does
            # nothing and the test passes for the wrong reason.
            lambda t: t.replace(
                "| 4 | GET | `/v1/me` | user | self | no | none | 200 |",
                "| 4 | GET | `/v1/me` | user | self | no | none | 299 |",
                1,
            ),
            "but contracts.md section 5.4 states",
        ),
        (
            "route defined in contracts.md that no api.md row lists",
            "contracts.md",
            lambda t: t.replace(
                "- Success `200`: `{id, display_name, status}`.",
                "- `GET /v1/undocumented-extra` — also served here.",
                1,
            ),
            "but no api.md row cites it",
        ),
        (
            "workflow asserting an exit code the tool does not define",
            ".github/workflows/clean-vm.yml",
            lambda t: t.replace("LASTEXITCODE -ne 10", "LASTEXITCODE -ne 99", 1),
            "EXIT_OPERATOR=10",
        ),
        (
            "workflow writing a BOM into $GITHUB_ENV",
            ".github/workflows/clean-vm.yml",
            lambda t: t.replace(
                "          Write-Host \"runner image : $env:ImageOS\"",
                "          \"image: $env:ImageOS\" | Out-File -FilePath "
                "\"$env:GITHUB_ENV\" -Append -Encoding utf8",
                1,
            ),
            "emits a UTF-8 BOM",
        ),
    ]

    failures = 0
    for name, filename, mutate, expect in defects:
        with tempfile.TemporaryDirectory() as tmp:
            tree = Path(tmp)
            for f in root.glob("*.md"):
                shutil.copy2(f, tree / f.name)
            # schema-draft.sql is copied too: without it the model/DDL check skips
            # itself and the two planted defects below would pass for the wrong reason.
            ddl_src = root / "schema-draft.sql"
            if ddl_src.exists():
                shutil.copy2(ddl_src, tree / ddl_src.name)
            # The workflow and the verification tool are needed too: without them the
            # workflow-consistency check skips itself and the planted exit-code defect
            # would pass for the wrong reason, which is the failure mode this self-test
            # exists to prevent.
            wf_src = root / ".github" / "workflows"
            if wf_src.is_dir():
                (tree / ".github" / "workflows").mkdir(parents=True, exist_ok=True)
                for f in wf_src.glob("*.yml"):
                    shutil.copy2(f, tree / ".github" / "workflows" / f.name)
            tool_src = root / "scripts" / "verify_clean_vm.py"
            if tool_src.exists():
                (tree / "scripts").mkdir(parents=True, exist_ok=True)
                shutil.copy2(tool_src, tree / "scripts" / tool_src.name)
            original = (tree / filename).read_text(encoding="utf-8-sig")
            (tree / filename).write_text(mutate(original), encoding="utf-8")
            errs, warns, _, _ = audit(tree)
            # A defect may surface as an error or a warning depending on severity,
            # so the planted defect is considered detected if either mentions it.
            found = errs + warns
            hit = any(expect in e for e in found)
            print(f"  {'DETECTED' if hit else 'MISSED  '}  {name}")
            if not hit:
                failures += 1
                print(f"           expected an error containing: {expect!r}")

    if failures:
        print(f"\nSELF-TEST FAILED: {failures} planted defect(s) went undetected")
        return 1
    print("\nSELF-TEST PASSED: every planted defect was detected")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--strict", action="store_true")
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args()

    if args.self_test:
        return self_test(ROOT)

    errors, warnings, stats, chain_text = audit(ROOT)
    for k, v in stats.items():
        print(f"{k:24}: {v}")
    print()
    print(f"longest chain:\n  {chain_text}")
    print()
    for w in warnings:
        print(f"WARN  {w}")
    for e in errors:
        print(f"ERROR {e}")

    if errors or (args.strict and warnings):
        print(f"\nFAILED: {len(errors)} error(s), {len(warnings)} warning(s)")
        return 1
    print(f"\nPASSED: 0 errors, {len(warnings)} warning(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())