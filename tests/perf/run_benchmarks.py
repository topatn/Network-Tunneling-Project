#!/usr/bin/env python3
"""Benchmark harness and baselines runner (T-095).

Requirements: TEST-006, PERF-001, PERF-002, PERF-003, PERF-004.
Authoritative targets: research.md ADR-023.

Executes:
1. Connection establishment time distribution (PERF-004).
2. Relay-added latency per transport (PERF-001).
3. Throughput per transport for bulk TCP and burst UDP (PERF-002).
4. Memory and flow footprint at idle, with 1 guest, and at saturation (PERF-003, NET-006).
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
from typing import Any


ROOT = Path(__file__).resolve().parent.parent.parent


def parse_benchmark_output(output: str) -> dict[str, Any]:
    """Parse benchmark metric output from cargo test."""
    results: dict[str, Any] = {
        "establishment": {},
        "latency": {"tcp": {}, "udp": {}},
        "throughput": {"tcp": {}, "udp": {}},
        "footprint": {"idle": {}, "single_guest": {}, "max_loaded": {}},
    }

    # PERF-004 Establishment
    # Min: 2.0 us \n Mean: 3.1 us \n p50: 2.1 us \n p95: 4.1 us \n p99: 6.8 us \n Max: 282.1 us
    est_block = re.search(
        r"\[PERF-004\] Connection Establishment Timing Distribution.*?"
        r"Min:\s*([\d\.]+)\s*us.*?"
        r"Mean:\s*([\d\.]+)\s*us.*?"
        r"p50:\s*([\d\.]+)\s*us.*?"
        r"p95:\s*([\d\.]+)\s*us.*?"
        r"p99:\s*([\d\.]+)\s*us.*?"
        r"Max:\s*([\d\.]+)\s*us",
        output,
        re.S,
    )
    if est_block:
        results["establishment"] = {
            "min_us": float(est_block.group(1)),
            "mean_us": float(est_block.group(2)),
            "p50_us": float(est_block.group(3)),
            "p95_us": float(est_block.group(4)),
            "p99_us": float(est_block.group(5)),
            "max_us": float(est_block.group(6)),
        }

    # PERF-001 Latency
    # TCP 64B RTT (100 samples): \n Min: 7.6 us | Mean: 18.9 us | p50: 17.7 us | p95: 32.7 us | p99: 34.1 us | Max: 34.1 us
    tcp_lat = re.search(
        r"TCP 64B RTT.*?"
        r"Min:\s*([\d\.]+)\s*us\s*\|\s*"
        r"Mean:\s*([\d\.]+)\s*us\s*\|\s*"
        r"p50:\s*([\d\.]+)\s*us\s*\|\s*"
        r"p95:\s*([\d\.]+)\s*us\s*\|\s*"
        r"p99:\s*([\d\.]+)\s*us\s*\|\s*"
        r"Max:\s*([\d\.]+)\s*us",
        output,
        re.S,
    )
    if tcp_lat:
        results["latency"]["tcp"] = {
            "min_us": float(tcp_lat.group(1)),
            "mean_us": float(tcp_lat.group(2)),
            "p50_us": float(tcp_lat.group(3)),
            "p95_us": float(tcp_lat.group(4)),
            "p99_us": float(tcp_lat.group(5)),
            "max_us": float(tcp_lat.group(6)),
        }

    # UDP 512B RTT (200 samples): \n Min: 7.2 us | Mean: 9.2 us | p50: 8.0 us | p95: 15.5 us | p99: 18.9 us | Max: 57.4 us
    udp_lat = re.search(
        r"UDP 512B RTT.*?"
        r"Min:\s*([\d\.]+)\s*us\s*\|\s*"
        r"Mean:\s*([\d\.]+)\s*us\s*\|\s*"
        r"p50:\s*([\d\.]+)\s*us\s*\|\s*"
        r"p95:\s*([\d\.]+)\s*us\s*\|\s*"
        r"p99:\s*([\d\.]+)\s*us\s*\|\s*"
        r"Max:\s*([\d\.]+)\s*us",
        output,
        re.S,
    )
    if udp_lat:
        results["latency"]["udp"] = {
            "min_us": float(udp_lat.group(1)),
            "mean_us": float(udp_lat.group(2)),
            "p50_us": float(udp_lat.group(3)),
            "p95_us": float(udp_lat.group(4)),
            "p99_us": float(udp_lat.group(5)),
            "max_us": float(udp_lat.group(6)),
        }

    # PERF-002 Throughput
    # TCP Bulk Throughput (4194304 bytes): \n Elapsed: 0.89 ms | Rate: 4513.14 MB/s
    tcp_tp = re.search(
        r"TCP Bulk Throughput \((\d+) bytes\):.*?"
        r"Elapsed:\s*([\d\.]+)\s*ms\s*\|\s*Rate:\s*([\d\.]+)\s*MB/s",
        output,
        re.S,
    )
    if tcp_tp:
        results["throughput"]["tcp"] = {
            "bytes": int(tcp_tp.group(1)),
            "elapsed_ms": float(tcp_tp.group(2)),
            "rate_mbps": float(tcp_tp.group(3)),
        }

    # UDP Datagram Rate (5000 pkts x 1200B): \n Elapsed: 1.33 ms | Rate: 3759964 pkts/s (4302.94 MB/s)
    udp_tp = re.search(
        r"UDP Datagram Rate \((\d+) pkts x (\d+)B\):.*?"
        r"Elapsed:\s*([\d\.]+)\s*ms\s*\|\s*Rate:\s*([\d\.]+)\s*pkts/s\s*\(([\d\.]+)\s*MB/s\)",
        output,
        re.S,
    )
    if udp_tp:
        results["throughput"]["udp"] = {
            "packets": int(udp_tp.group(1)),
            "packet_size": int(udp_tp.group(2)),
            "elapsed_ms": float(udp_tp.group(3)),
            "rate_pkts_sec": float(udp_tp.group(4)),
            "rate_mbps": float(udp_tp.group(5)),
        }

    # PERF-003 Footprint
    # Idle State: Active TCP Flows: 0 | Active UDP Flows: 0 | Queued: 0 B
    idle = re.search(
        r"Idle State:\s*Active TCP Flows:\s*(\d+)\s*\|\s*Active UDP Flows:\s*(\d+)\s*\|\s*Queued:\s*(\d+)\s*B",
        output,
    )
    if idle:
        results["footprint"]["idle"] = {
            "active_tcp_flows": int(idle.group(1)),
            "active_udp_flows": int(idle.group(2)),
            "queued_bytes": int(idle.group(3)),
        }

    # 1-Guest State: Active TCP Flows: 1 | Active UDP Flows: 1
    single = re.search(
        r"1-Guest State:\s*Active TCP Flows:\s*(\d+)\s*\|\s*Active UDP Flows:\s*(\d+)",
        output,
    )
    if single:
        results["footprint"]["single_guest"] = {
            "active_tcp_flows": int(single.group(1)),
            "active_udp_flows": int(single.group(2)),
        }

    # Max Loaded State (Saturation): Active TCP Flows: 64/64 | Active UDP Flows: 64/64 \n Session Queue Reserved: 131072 B (128.0 KB) \n Global Budget Usage: 131072 B (128.0 KB)
    max_load = re.search(
        r"Max Loaded State \(Saturation\):.*?"
        r"Active TCP Flows:\s*(\d+)/(\d+)\s*\|\s*Active UDP Flows:\s*(\d+)/(\d+).*?"
        r"Session Queue Reserved:\s*(\d+)\s*B.*?"
        r"Global Budget Usage:\s*(\d+)\s*B",
        output,
        re.S,
    )
    if max_load:
        results["footprint"]["max_loaded"] = {
            "active_tcp_flows": int(max_load.group(1)),
            "max_tcp_flows": int(max_load.group(2)),
            "active_udp_flows": int(max_load.group(3)),
            "max_udp_flows": int(max_load.group(4)),
            "session_queue_reserved_bytes": int(max_load.group(5)),
            "global_budget_bytes": int(max_load.group(6)),
        }

    return results


def run_cargo_benchmarks() -> tuple[bool, str]:
    """Execute the Rust performance benchmark test."""
    cargo_bin = shutil.which("cargo") or str(Path.home() / ".cargo" / "bin" / "cargo.exe")
    cmd = [cargo_bin]
    if sys.platform == "win32":
        cmd.append("+stable-x86_64-pc-windows-gnu")
    cmd.extend([
        "test",
        "-p",
        "gtt-engine",
        "--test",
        "perf_benchmarks",
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
            and "test result: ok. 4 passed" in proc.stdout
        )
        combined_output = proc.stdout + ("\n" + proc.stderr if proc.stderr else "")
        return passed, combined_output
    except Exception as exc:
        return False, f"Failed to execute benchmark tests: {exc}"


def verify_targets(metrics: dict[str, Any]) -> list[tuple[str, str, bool, str]]:
    """Assert metrics against ADR-023 target thresholds."""
    checks = []

    # Target 1: Connection establishment p95 <= 100 ms (100,000 us)
    est_p95 = metrics.get("establishment", {}).get("p95_us", 0.0)
    passed = 0.0 < est_p95 <= 100_000.0
    checks.append((
        "PERF-004",
        "Connection Establishment Time (p95)",
        passed,
        f"{est_p95:.2f} us (target: <= 100,000.00 us / 100 ms)",
    ))

    # Target 2: Relay forwarding added latency (p95 <= 5 ms / 5,000 us)
    tcp_lat_p95 = metrics.get("latency", {}).get("tcp", {}).get("p95_us", 0.0)
    passed = 0.0 < tcp_lat_p95 <= 5_000.0
    checks.append((
        "PERF-001",
        "Relay Added Latency TCP RTT (p95)",
        passed,
        f"{tcp_lat_p95:.2f} us (target: <= 5,000.00 us / 5 ms)",
    ))

    udp_lat_p95 = metrics.get("latency", {}).get("udp", {}).get("p95_us", 0.0)
    passed = 0.0 < udp_lat_p95 <= 5_000.0
    checks.append((
        "PERF-001",
        "Relay Added Latency UDP RTT (p95)",
        passed,
        f"{udp_lat_p95:.2f} us (target: <= 5,000.00 us / 5 ms)",
    ))

    # Target 3: Single-stream TCP throughput (>= 25 MB/s)
    tcp_rate = metrics.get("throughput", {}).get("tcp", {}).get("rate_mbps", 0.0)
    passed = tcp_rate >= 25.0
    checks.append((
        "PERF-002",
        "TCP Bulk Stream Throughput",
        passed,
        f"{tcp_rate:.2f} MB/s (target: >= 25.00 MB/s)",
    ))

    # Target 4: UDP datagram burst rate (>= 10,000 pkts/s)
    udp_rate = metrics.get("throughput", {}).get("udp", {}).get("rate_pkts_sec", 0.0)
    passed = udp_rate >= 10_000.0
    checks.append((
        "PERF-002",
        "UDP Datagram Rate",
        passed,
        f"{udp_rate:.0f} pkts/s (target: >= 10,000 pkts/s)",
    ))

    # Target 5: Flow table memory bounds under saturation (NET-006, PERF-003)
    max_q = metrics.get("footprint", {}).get("max_loaded", {}).get("session_queue_reserved_bytes", 0)
    passed = 0 < max_q <= 1_048_576  # 1 MB bound
    checks.append((
        "PERF-003",
        "Session Queue Footprint under Saturation",
        passed,
        f"{max_q} B ({max_q / 1024:.1f} KB, target: <= 1,048,576 B / 1 MB)",
    ))

    return checks


def main() -> int:
    parser = argparse.ArgumentParser(description="Run performance benchmark harness (T-095)")
    parser.add_argument("--json-out", type=Path, default=None, help="Output path for JSON metrics")
    parser.add_argument("--verbose", action="store_true", help="Print full cargo output")
    args = parser.parse_args()

    print("=" * 70)
    print(" Running Performance Benchmark Suite (T-095)")
    print(" Requirements: TEST-006, PERF-001, PERF-002, PERF-003, PERF-004")
    print("=" * 70)

    start_time = time.monotonic()
    success, output = run_cargo_benchmarks()
    elapsed = time.monotonic() - start_time

    if args.verbose or not success:
        print(output)

    if not success:
        print(f"\n[FAIL] Benchmark suite execution failed (elapsed: {elapsed:.2f}s)")
        return 1

    metrics = parse_benchmark_output(output)

    if args.json_out:
        args.json_out.parent.mkdir(parents=True, exist_ok=True)
        with open(args.json_out, "w", encoding="utf-8") as f:
            json.dump(metrics, f, indent=2)
        print(f"Metrics written to: {args.json_out}")

    print("\nBenchmark Assertions vs Targets (ADR-023):")
    print("-" * 70)
    all_passed = True
    for req, desc, passed, detail in verify_targets(metrics):
        status = "[PASS]" if passed else "[FAIL]"
        if not passed:
            all_passed = False
        print(f"  {status} {req} {desc:<42} : {detail}")
    print("-" * 70)

    if all_passed:
        print(f"\n[PASS] All performance targets satisfied. Suite elapsed: {elapsed:.2f}s")
        return 0
    else:
        print(f"\n[FAIL] One or more performance targets failed. Suite elapsed: {elapsed:.2f}s")
        return 1


if __name__ == "__main__":
    sys.exit(main())
