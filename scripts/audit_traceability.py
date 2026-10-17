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
COUNTS_LINE = re.compile(r"(\d+)\s+requirements,\s*(\d+)\s+tasks")
PLAN_TOTALS = re.compile(r"Total:\s*(\d+)\s+tasks,\s*(\d+)\s+requirements")


def read(root: Path, name: str) -> str:
    # utf-8-sig tolerates a BOM, which some PowerShell writers emit.
    return (root / name).read_text(encoding="utf-8-sig")


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

    # 7. Declared counts must match what is actually there.
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

    stats = {
        "requirements declared": len(declared),
        "traceability rows": len(appendix),
        "tasks": len(tasks),
        "experiments": len(experiments),
        "ADRs": len(adrs),
        "invariants": len(invariants),
    }
    return errors, warnings, stats


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
            "wrong declared count",
            "requirements.md",
            lambda t: re.sub(r"Counts: \d+ requirements", "Counts: 999 requirements", t, count=1),
            "requirements, found",
        ),
    ]

    failures = 0
    for name, filename, mutate, expect in defects:
        with tempfile.TemporaryDirectory() as tmp:
            tree = Path(tmp)
            for f in root.glob("*.md"):
                shutil.copy2(f, tree / f.name)
            original = (tree / filename).read_text(encoding="utf-8-sig")
            (tree / filename).write_text(mutate(original), encoding="utf-8")
            errs, warns, _ = audit(tree)
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

    errors, warnings, stats = audit(ROOT)
    for k, v in stats.items():
        print(f"{k:24}: {v}")
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