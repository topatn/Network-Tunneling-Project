#!/usr/bin/env python3
"""
Comprehensive automated verification script for Game Compatibility & Managed Server Verification
(T-090, T-035, T-076, TEST-005, GAME-001..GAME-009, SRV-001..SRV-008, SRV-015, SRV-018).

Runs:
1. gtt-server-manager unit and isolation tests (31 tests in server_manager.rs).
2. gtt-engine game compatibility integration tests (4 tests in game_compat.rs).
3. gtt-engine managed server lifecycle tests (2 tests in managed_server_lifecycle.rs).
4. Standalone L0/L1 game compatibility and gap inventory runner (tests/game-compat/run_game_compat.py).
5. Zero-warning Clippy check on gtt-server-manager and gtt-engine (--all-targets -- -D warnings).
6. Traceability audit self-test and workspace audit.
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
    print(" Game Compatibility & Managed Server Verification (T-090, T-035)")
    print("================================================================================")

    env = build_rust_env()
    cargo = cargo_cmd_prefix(env)

    # 1. gtt-server-manager unit & isolation tests (T-035)
    run_step(
        cargo + ["test", "-p", "gtt-server-manager"],
        "T-035: gtt-server-manager unit & isolation tests (31 tests)",
        env=env,
    )

    # 2. gtt-engine game compatibility integration tests (T-090)
    run_step(
        cargo + ["test", "-p", "gtt-engine", "--test", "game_compat"],
        "T-090: gtt-engine game compatibility integration tests (4 tests)",
        env=env,
    )

    # 3. gtt-engine managed server lifecycle tests (T-076, T-035)
    run_step(
        cargo + ["test", "-p", "gtt-engine", "--test", "managed_server_lifecycle"],
        "T-076 / T-035: gtt-engine managed server lifecycle tests (2 tests)",
        env=env,
    )

    # 4. Standalone L0/L1 game compatibility & gap inventory runner
    run_step(
        [sys.executable, "tests/game-compat/run_game_compat.py"],
        "T-090: Standalone L0/L1 game compatibility & gap inventory runner",
        env=env,
    )

    # 5. Quality: Clippy zero-warning check on gtt-server-manager and gtt-engine
    run_step(
        cargo + ["clippy", "-p", "gtt-server-manager", "-p", "gtt-engine", "--all-targets", "--", "-D", "warnings"],
        "Quality: Clippy zero-warning check on gtt-server-manager and gtt-engine",
        env=env,
    )

    # 6. Traceability: Audit self-test
    run_step(
        [sys.executable, "scripts/audit_traceability.py", "--self-test"],
        "Traceability: Audit harness self-test",
        env=env,
    )

    # 7. Traceability: Audit requirements and task mapping
    run_step(
        [sys.executable, "scripts/audit_traceability.py"],
        "Traceability: Comprehensive requirements and task audit (0 errors)",
        env=env,
    )

    print("\n================================================================================")
    print(" ALL GAME COMPATIBILITY & MANAGED SERVER CHECKS PASSED (EXIT 0)")
    print("================================================================================")


if __name__ == "__main__":
    main()
