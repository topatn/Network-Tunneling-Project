#!/usr/bin/env python3
"""
Comprehensive automated verification script for Network Impairment & Resilience Verification
(T-092, TEST-003, NET-004, NET-005, NET-006, NET-007, NET-008, NET-010, FR-008, FR-014).

Runs:
1. gtt-engine network impairment integration tests (crates/engine/tests/network_impairment.rs).
2. gtt-engine session resumption tests (crates/engine/tests/session_resumption.rs).
3. gtt-engine failure injection tests (crates/engine/tests/failure_injection.rs).
4. gtt-relay session limits and queue bounds tests (crates/relay/tests/session_and_limits.rs).
5. gtt-relay shutdown and crash cleanup tests (crates/relay/tests/shutdown_cleanup.rs).
6. gtt-relay TCP and UDP forwarding tests (crates/relay/tests/tcp_forwarding.rs, udp_forwarding.rs).
7. Standalone Python network impairment & IPv6 harness (tests/network/run_network_tests.py).
8. Quality: Clippy zero-warning check on gtt-engine and gtt-relay.
9. Traceability: Audit harness self-test and workspace audit.
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
    print(" Network Impairment & Resilience Verification (T-092, TEST-003, NET-005)")
    print("================================================================================")

    env = build_rust_env()
    cargo = cargo_cmd_prefix(env)

    # 1. gtt-engine network impairment tests (T-092, TEST-003, NET-005, NET-010)
    run_step(
        cargo + ["test", "-p", "gtt-engine", "--test", "network_impairment"],
        "T-092: gtt-engine network impairment suite (5 tests: latency, packet loss, blocked UDP, resumption, IPv4/IPv6)",
        env=env,
    )

    # 2. gtt-engine session resumption tests (NET-004, FR-014)
    run_step(
        cargo + ["test", "-p", "gtt-engine", "--test", "session_resumption"],
        "NET-004 / FR-014: gtt-engine session resumption within grace window",
        env=env,
    )

    # 3. gtt-engine failure injection tests (REL-002, REL-003)
    run_step(
        cargo + ["test", "-p", "gtt-engine", "--test", "failure_injection"],
        "REL-002 / REL-003: gtt-engine failure injection (relay crash, CP outage, failover)",
        env=env,
    )

    # 4. gtt-relay session limits and queue bounds (SEC-015, PERF-003, NET-006)
    run_step(
        cargo + ["test", "-p", "gtt-relay", "--test", "session_and_limits"],
        "SEC-015 / PERF-003: gtt-relay session limits, queue bounding, and idle timeout",
        env=env,
    )

    # 5. gtt-relay shutdown and cleanup (NET-008, INV-6)
    run_step(
        cargo + ["test", "-p", "gtt-relay", "--test", "shutdown_cleanup"],
        "NET-008 / INV-6: gtt-relay shutdown port-scan verification and crash orphan cleanup",
        env=env,
    )

    # 6. gtt-relay forwarding integrity (NET-001, NET-003)
    run_step(
        cargo + ["test", "-p", "gtt-relay", "--test", "tcp_forwarding", "--test", "udp_forwarding"],
        "NET-001 / NET-003: gtt-relay bidirectional TCP/UDP forwarding with inbound host blocked",
        env=env,
    )

    # 7. Standalone Python Network Impairment & IPv6 Harness
    run_step(
        [sys.executable, "tests/network/run_network_tests.py"],
        "T-092: Standalone synthetic network impairment and IPv4/IPv6 mismatch runner",
        env=env,
    )

    # 8. Quality: Clippy zero-warning check on gtt-engine and gtt-relay
    run_step(
        cargo + ["clippy", "-p", "gtt-engine", "-p", "gtt-relay", "--all-targets", "--", "-D", "warnings"],
        "Quality: Clippy zero-warning check on gtt-engine and gtt-relay",
        env=env,
    )

    # 9. Traceability: Audit self-test and workspace audit
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
    print(" ALL NETWORK IMPAIRMENT & RESILIENCE CHECKS PASSED (EXIT 0)")
    print("================================================================================")


if __name__ == "__main__":
    main()
