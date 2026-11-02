#!/usr/bin/env python3
"""Relay load test and capacity limits automation runner (T-096).

Requirements: PERF-005, OPS-004, NET-006.
Authoritative targets: docs/relay-load-test-t096.md, research.md ADR-023.

Exercises and asserts:
1. Concurrent allocation scaling and instance capacity ceiling (PERF-005, OPS-004).
2. Per-session flow saturation (64 TCP / 64 UDP) and deterministic drop counting (NET-006).
3. Global relay memory flood, buffer cap, and backpressure drop reasons (NET-006, OPS-004).
4. Token-bucket bandwidth rate limiting, burst consumption, and continuous refill (OPS-004).
5. High-density state lookup (< 10 us/session) and grace sweep performance (< 50 ms for 250 allocations) (PERF-005).
"""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
from typing import Any


ROOT = Path(__file__).resolve().parent.parent.parent


def run_cargo_load_tests() -> tuple[bool, str]:
    """Execute the Rust load capacity test suite."""
    cargo_bin = shutil.which("cargo") or str(Path.home() / ".cargo" / "bin" / "cargo.exe")
    cmd = [cargo_bin]
    if sys.platform == "win32":
        cmd.append("+stable-x86_64-pc-windows-gnu")
    cmd.extend([
        "test",
        "-p",
        "gtt-relay",
        "--test",
        "load_capacity",
        "--",
        "--nocapture",
    ])

    try:
        proc = subprocess.run(
            cmd,
            cwd=ROOT,
            capture_output=True,
            text=True,
            timeout=180,
        )
        passed = (
            proc.returncode == 0
            and "test result: ok. 5 passed" in proc.stdout
        )
        combined_output = proc.stdout + ("\n" + proc.stderr if proc.stderr else "")
        return passed, combined_output
    except Exception as exc:
        return False, f"Failed to execute relay load capacity test: {exc}"


def verify_load_report(output: str) -> list[tuple[str, str, bool, str]]:
    """Parse output and verify load metrics against configured limits."""
    checks = []

    # 1. Allocation scaling and ceiling (PERF-005)
    alloc_passed = (
        "Successfully scaled 50 allocations" in output
        and "test test_concurrent_allocation_scaling_and_capacity_ceiling ... ok" in output
    )
    checks.append((
        "PERF-005",
        "Allocation Scaling and Capacity Ceiling (max_allocations = 250)",
        alloc_passed,
        "50 scaled, 51st rejected with MaxAllocationsReached; capacity released on stop",
    ))

    # 2. Flow saturation (NET-006)
    flow_passed = (
        "Flow saturation verified: 64/64 TCP and 64/64 UDP held" in output
        and "test test_concurrent_flow_saturation_and_limit_enforcement ... ok" in output
    )
    checks.append((
        "NET-006",
        "Per-Session Flow Limits (64 TCP / 64 UDP)",
        flow_passed,
        "64/64 flows accepted; 65th TCP rejected with FlowLimitReached; 65th UDP dropped with dropped_flow_limit = 1",
    ))

    # 3. Global memory backpressure (NET-006, OPS-004)
    mem_passed = (
        "Global budget saturated to capacity" in output
        and "test test_global_relay_memory_saturation_and_backpressure_drop ... ok" in output
    )
    checks.append((
        "NET-006 / OPS-004",
        "Global Memory Cap and Backpressure Queueing",
        mem_passed,
        "Saturated at 64 KiB; overflow dropped with GlobalMemoryCapReached; zero memory leak on clear",
    ))

    # 4. Bandwidth token bucket (OPS-004)
    bw_passed = (
        "Bandwidth token bucket burst exhaustion and continuous refill verified" in output
        and "test test_bandwidth_saturation_and_refill_recovery ... ok" in output
    )
    checks.append((
        "OPS-004",
        "Token-Bucket Bandwidth Limiter (2 MiB/s session rate)",
        bw_passed,
        "Burst exhausted with BandwidthLimitExceeded; continuous token refill confirmed on time advance",
    ))

    # 5. High-density state benchmark (PERF-005)
    lookup_m = re.search(r"Lookup Time:\s*([\d\.]+)\s*us avg per allocation", output)
    sweep_m = re.search(r"Sweep Time \(50 evicted\):\s*([\d\.]+)\s*ms", output)
    hd_passed = (
        lookup_m is not None
        and float(lookup_m.group(1)) < 10.0
        and sweep_m is not None
        and float(sweep_m.group(1)) < 50.0
        and "test test_high_density_state_sweep_performance ... ok" in output
    )
    lookup_str = f"{lookup_m.group(1)} us/alloc" if lookup_m else "N/A"
    sweep_str = f"{sweep_m.group(1)} ms" if sweep_m else "N/A"
    checks.append((
        "PERF-005",
        "High-Density State Management (250 Allocations)",
        hd_passed,
        f"Lookup: {lookup_str} (target < 10 us); Sweep: {sweep_str} (target < 50 ms)",
    ))

    return checks


def main() -> int:
    parser = argparse.ArgumentParser(description="Run relay load capacity harness (T-096)")
    parser.add_argument("--verbose", action="store_true", help="Print full cargo output")
    args = parser.parse_args()

    print("=" * 70)
    print(" Running Relay Load Test and Capacity Verification (T-096)")
    print(" Requirements: PERF-005, OPS-004, NET-006")
    print("=" * 70)

    start_time = time.monotonic()
    success, output = run_cargo_load_tests()
    elapsed = time.monotonic() - start_time

    if args.verbose or not success:
        print(output)

    if not success:
        print(f"\n[FAIL] Relay load test execution failed (elapsed: {elapsed:.2f}s)")
        return 1

    checks = verify_load_report(output)

    print("\nRelay Capacity & Saturation Assertions:")
    print("-" * 70)
    all_passed = True
    for req, desc, passed, detail in checks:
        status = "[PASS]" if passed else "[FAIL]"
        if not passed:
            all_passed = False
        print(f"  {status} {req:<18} {desc:<40} : {detail}")
    print("-" * 70)

    if all_passed:
        print(f"\n[PASS] All relay capacity and load targets satisfied. Elapsed: {elapsed:.2f}s")
        return 0
    else:
        print(f"\n[FAIL] One or more capacity targets failed. Elapsed: {elapsed:.2f}s")
        return 1


if __name__ == "__main__":
    sys.exit(main())
