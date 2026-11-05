#!/usr/bin/env python3
"""
Comprehensive automated verification script for Client-Side Tunnel Session & Engine Integration
(T-070, T-073, T-080, T-083, contracts.md §12, §13).

Runs:
1. gtt-desktop unit and integration tests (credential store, packaging lifecycle, engine bridge).
2. gtt-engine lifecycle, target policy, local probes, and players-seen tests.
3. gtt-protocol schemas and engine session state machine tests.
4. Full desktop frontend test suite (8 suites).
5. Design-taste frontend linter (0 FAIL, 0 WARN).
6. Strict WCAG AA contrast check (UX-005).
7. Zero-warning Clippy check on gtt-desktop, gtt-engine, and gtt-protocol.
8. Traceability audit with self-test.
"""

from __future__ import annotations

import os
import pathlib
import shutil
import subprocess
import sys

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent


def build_rust_env() -> dict[str, str]:
    env = os.environ.copy()
    if sys.platform == "win32":
        home = pathlib.Path.home()
        gnu_bin = home / ".rustup" / "toolchains" / "stable-x86_64-pc-windows-gnu" / "lib" / "rustlib" / "x86_64-pc-windows-gnu" / "bin"
        self_contained = gnu_bin / "self-contained"
        cargo_bin = home / ".cargo" / "bin"
        extra = [str(p) for p in (cargo_bin, gnu_bin, self_contained) if p.exists()]
        if extra:
            env["PATH"] = os.pathsep.join(extra + [env.get("PATH", "")])
    return env


def cargo_cmd_prefix(env: dict[str, str]) -> list[str]:
    cargo_exe = shutil.which("cargo", path=env.get("PATH")) or "cargo"
    if sys.platform == "win32":
        home = pathlib.Path.home()
        gnu_toolchain = home / ".rustup" / "toolchains" / "stable-x86_64-pc-windows-gnu"
        if gnu_toolchain.exists():
            return [cargo_exe, "+stable-x86_64-pc-windows-gnu"]
    return [cargo_exe]


def run_step(cmd: list[str], description: str, env: dict[str, str] | None = None, cwd: pathlib.Path | None = None) -> None:
    print(f"\n--- [RUNNING] {description} ---")
    print(" ".join(cmd))
    res = subprocess.run(cmd, env=env, cwd=cwd or REPO_ROOT, text=True)
    if res.returncode != 0:
        print(f"[FAILED] {description} exited with code {res.returncode}", file=sys.stderr)
        sys.exit(res.returncode)
    print(f"[OK] {description} passed.")


def main() -> None:
    print("================================================================================")
    print(" Client-Side Tunnel Session & Engine Integration Verification (T-070, T-073)")
    print("================================================================================")

    env = build_rust_env()
    cargo = cargo_cmd_prefix(env)
    node_exe = shutil.which("node", path=env.get("PATH")) or "node"
    npm_cmd = shutil.which("npm", path=env.get("PATH")) or "npm"

    # 1. gtt-desktop tests
    run_step(
        cargo + ["test", "-p", "gtt-desktop"],
        "T-070 / T-073 / T-080: gtt-desktop tests (engine bridge, packaging, credentials)",
        env=env,
    )

    # 2. gtt-engine tests
    run_step(
        cargo + ["test", "-p", "gtt-engine", "--test", "lifecycle_integration", "--test", "target_policy", "--test", "local_probes", "--test", "players_seen"],
        "T-070 / T-071 / T-072 / T-073: gtt-engine tests (lifecycle, target policy, probes, players seen)",
        env=env,
    )

    # 3. gtt-protocol tests
    run_step(
        cargo + ["test", "-p", "gtt-protocol"],
        "contracts.md §12: gtt-protocol session state machine and framing tests",
        env=env,
    )

    # 4. Desktop frontend test harness (8 suites)
    run_step(
        [npm_cmd, "test", "--prefix", "apps/desktop"],
        "T-080..T-088: Full desktop frontend test harness (8 suites)",
        env=env,
    )

    # 5. Quality: Design-taste frontend linter
    linter_path = pathlib.Path.home() / ".gemini" / "config" / "skills" / "design-taste-frontend" / "scripts" / "lint.mjs"
    if linter_path.exists():
        run_step(
            [node_exe, str(linter_path), "--app", "apps/desktop"],
            "Quality: Design-taste frontend linter (0 FAIL, 0 WARN)",
            env=env,
        )

    # 6. Quality: Strict WCAG AA contrast check
    run_step(
        [sys.executable, "scripts/check_contrast.py", "--strict"],
        "Quality: Strict WCAG AA contrast check (UX-005)",
        env=env,
    )

    # 7. Quality: Clippy zero-warning check on gtt-desktop, gtt-engine, and gtt-protocol
    run_step(
        cargo + ["clippy", "-p", "gtt-desktop", "-p", "gtt-engine", "-p", "gtt-protocol", "--all-targets", "--", "-D", "warnings"],
        "Quality: Clippy zero-warning check on gtt-desktop, gtt-engine, and gtt-protocol",
        env=env,
    )

    # 8. Traceability: Audit requirements and task mapping
    run_step(
        [sys.executable, "scripts/audit_traceability.py"],
        "Traceability: Comprehensive requirements and task audit (0 errors)",
        env=env,
    )

    print("\n================================================================================")
    print(" ALL CLIENT-SIDE TUNNEL SESSION & ENGINE CHECKS PASSED (EXIT 0)")
    print("================================================================================")


if __name__ == "__main__":
    main()
