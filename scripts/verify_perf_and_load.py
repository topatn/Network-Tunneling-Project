#!/usr/bin/env python3
"""
Comprehensive automated verification script for Performance Benchmarks & Relay Capacity Limits
(T-095, T-096, TEST-006, PERF-001, PERF-002, PERF-003, PERF-004, PERF-005, OPS-004, NET-006, ADR-023).

Runs:
1. gtt-engine performance benchmarks suite (crates/engine/tests/perf_benchmarks.rs).
2. gtt-relay load capacity and saturation suite (crates/relay/tests/load_capacity.rs).
3. Standalone Python performance benchmark target runner (tests/perf/run_benchmarks.py).
4. Standalone Python relay load test and capacity limits runner (tests/perf/run_relay_load.py).
5. Quality: Clippy zero-warning check on gtt-engine and gtt-relay.
6. Traceability: Audit harness self-test and workspace audit.
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
    print(" Performance Benchmarks & Relay Capacity Verification (T-095, T-096)")
    print("================================================================================")

    env = build_rust_env()
    cargo = cargo_cmd_prefix(env)

    # 1. gtt-engine perf benchmarks suite (T-095, TEST-006, PERF-001..PERF-004)
    run_step(
        cargo + ["test", "-p", "gtt-engine", "--test", "perf_benchmarks", "--", "--nocapture"],
        "T-095: gtt-engine performance benchmarks suite (establishment, latency, throughput, footprint)",
        env=env,
    )

    # 2. gtt-relay load capacity suite (T-096, PERF-005, OPS-004, NET-006)
    run_step(
        cargo + ["test", "-p", "gtt-relay", "--test", "load_capacity", "--", "--nocapture"],
        "T-096: gtt-relay load capacity & saturation suite (scaling, flow caps, memory, bandwidth, sweep)",
        env=env,
    )

    # 3. Standalone Python performance benchmark target runner
    run_step(
        [sys.executable, "tests/perf/run_benchmarks.py"],
        "T-095: Standalone performance benchmark assertion runner (ADR-023 baselines)",
        env=env,
    )

    # 4. Standalone Python relay load test runner
    run_step(
        [sys.executable, "tests/perf/run_relay_load.py"],
        "T-096: Standalone relay capacity and saturation assertion runner",
        env=env,
    )

    # 5. Quality: Clippy zero-warning check on gtt-engine and gtt-relay
    run_step(
        cargo + ["clippy", "-p", "gtt-engine", "-p", "gtt-relay", "--all-targets", "--", "-D", "warnings"],
        "Quality: Clippy zero-warning check on gtt-engine and gtt-relay",
        env=env,
    )

    # 6. Traceability: Audit self-test and workspace audit
    run_step(
        [sys.executable, "scripts/audit_traceability.py", "--self-test"],
        "Traceability: Audit harness self-test",
        env=env,
    )
    run_step(
        [sys.executable, "scripts/audit_traceability.py"],
        "Traceability: Comprehensive requirements and task audit (0 errors)",
        env=env,
    )

    print("\n================================================================================")
    print(" ALL PERFORMANCE & RELAY CAPACITY CHECKS PASSED (EXIT 0)")
    print("================================================================================")


if __name__ == "__main__":
    main()
