#!/usr/bin/env python3
"""Privacy automated test runner and verification harness (T-098).

Requirements: PRIV-001, PRIV-003, PRIV-004.
Authoritative rules: AGENTS.md §7, contracts.md §5.13, requirements.md.

Exercises:
1. Default-install network capture verifying zero telemetry or unauthorized outbound traffic (PRIV-001).
2. Workspace dependencies audit asserting zero telemetry/analytics libraries (PRIV-001).
3. Codebase static scan ensuring no third-party tracking domains or endpoints exist (PRIV-001).
4. Data minimization and structured log redaction verifying zero raw IPs, secrets, or tokens (PRIV-002).
5. Data retention schedule verification across all 7 data categories (PRIV-003).
6. User account deletion verifying immediate resource termination, revocation, and cascading purge (PRIV-004).
"""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
from typing import NamedTuple


class ScenarioResult(NamedTuple):
    scenario_id: str
    requirement: str
    description: str
    passed: bool
    details: str


BANNED_TELEMETRY_CRATES = [
    "sentry",
    "datadog",
    "segment",
    "mixpanel",
    "amplitude",
    "posthog",
    "google-analytics",
    "appcenter",
    "telemetry-sdk",
    "statsig",
    "launchdarkly",
    "split-io",
]

BANNED_TRACKING_DOMAINS = [
    r"google-analytics\.com",
    r"analytics\.google\.com",
    r"sentry\.io",
    r"datadoghq\.com",
    r"segment\.io",
    r"api\.segment\.io",
    r"mixpanel\.com",
    r"amplitude\.com",
    r"api\.posthog\.com",
    r"telemetry\.",
    r"crashlytics\.com",
]


def setup_environment() -> dict[str, str]:
    """Ensure PATH includes required toolchain binary directories."""
    env = os.environ.copy()
    if sys.platform == "win32":
        rustup_base = (
            Path.home()
            / ".rustup"
            / "toolchains"
            / "stable-x86_64-pc-windows-gnu"
            / "lib"
            / "rustlib"
            / "x86_64-pc-windows-gnu"
            / "bin"
        )
        self_contained = rustup_base / "self-contained"
        paths_to_add = [str(rustup_base), str(self_contained)]
        existing_path = env.get("PATH", "")
        env["PATH"] = os.pathsep.join(paths_to_add + [existing_path])
    return env


def run_cargo_test(package: str, test_name: str, filter_str: str | None = None) -> tuple[bool, str]:
    """Execute a cargo test target and return (passed, output)."""
    cargo_bin = shutil.which("cargo") or str(Path.home() / ".cargo" / "bin" / "cargo.exe")
    cmd = [cargo_bin]
    if sys.platform == "win32":
        cmd.append("+stable-x86_64-pc-windows-gnu")
    cmd.extend(["test", "-p", package, "--test", test_name])
    if filter_str:
        cmd.extend(["--", filter_str, "--nocapture"])
    else:
        cmd.extend(["--", "--nocapture"])

    env = setup_environment()
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=120,
            env=env,
        )
        passed = proc.returncode == 0 and "test result: ok." in proc.stdout
        output = proc.stdout if passed else f"{proc.stdout}\n{proc.stderr}"
        return passed, output
    except Exception as exc:
        return False, f"Failed to execute cargo test ({package} / {test_name}): {exc}"


def test_network_capture_zero_telemetry() -> ScenarioResult:
    """Verify default-install engine emits zero telemetry and communicates only with configured targets."""
    passed, output = run_cargo_test("gtt-engine", "privacy_network_capture", "test_default_install_network_capture_has_zero_telemetry")
    return ScenarioResult(
        scenario_id="PRIV-SCENARIO-01",
        requirement="PRIV-001",
        description="Default-install network capture verifies 100% of HTTP traffic is strictly to configured API URL with zero telemetry",
        passed=passed,
        details=output if not passed else "Verified HTTP client connects exclusively to configured API endpoint with zero telemetry headers or beacon endpoints.",
    )


def test_lifecycle_probes_strictly_scoped() -> ScenarioResult:
    """Verify engine probes only connect to localhost and the explicit relay."""
    passed, output = run_cargo_test("gtt-engine", "privacy_network_capture", "test_engine_lifecycle_probes_only_loopback_and_configured_relay")
    return ScenarioResult(
        scenario_id="PRIV-SCENARIO-02",
        requirement="PRIV-001, SEC-006",
        description="Engine lifecycle reachability probes strictly restricted to local loopback (127.0.0.1/::1) and configured relay endpoint",
        passed=passed,
        details=output if not passed else "Verified local reachability probes dial only loopback/Tailnet addresses and relay control dials only configured relay.",
    )


def test_dependency_register_no_telemetry_crates() -> ScenarioResult:
    """Scan Cargo.lock and dependency register to assert zero analytics or tracking libraries."""
    passed, output = run_cargo_test("gtt-engine", "privacy_network_capture", "test_no_telemetry_crates_in_workspace_dependencies")
    if not passed:
        return ScenarioResult(
            scenario_id="PRIV-SCENARIO-03",
            requirement="PRIV-001",
            description="Cargo.lock and workspace dependency tree contains zero tracking or analytics SDKs",
            passed=False,
            details=output,
        )

    # Double check by scanning Cargo.lock directly in Python
    lock_path = Path("Cargo.lock")
    if lock_path.exists():
        content = lock_path.read_text(encoding="utf-8")
        found_crates = [crate for crate in BANNED_TELEMETRY_CRATES if f'name = "{crate}"' in content]
        if found_crates:
            return ScenarioResult(
                scenario_id="PRIV-SCENARIO-03",
                requirement="PRIV-001",
                description="Cargo.lock contains zero tracking or analytics SDKs",
                passed=False,
                details=f"Banned telemetry crates detected in Cargo.lock: {found_crates}",
            )

    return ScenarioResult(
        scenario_id="PRIV-SCENARIO-03",
        requirement="PRIV-001",
        description="Cargo.lock and workspace dependencies verified free of telemetry SDKs",
        passed=True,
        details="Scanned Cargo.lock and workspace manifests: 0 telemetry crates found.",
    )


def test_codebase_zero_tracking_domains() -> ScenarioResult:
    """Scan all Rust source code for banned third-party tracking and telemetry endpoints."""
    violations: list[str] = []
    crates_dir = Path("crates")

    for rs_file in crates_dir.rglob("*.rs"):
        # Skip this test file or mock diagnostics that test the leak scanner
        if "diagnostics_bundle.rs" in rs_file.name or "privacy_network_capture.rs" in rs_file.name:
            continue
        try:
            content = rs_file.read_text(encoding="utf-8")
            for pattern in BANNED_TRACKING_DOMAINS:
                if re.search(pattern, content, re.IGNORECASE):
                    violations.append(f"{rs_file}: matched banned domain pattern '{pattern}'")
        except Exception as exc:
            violations.append(f"{rs_file}: read error {exc}")

    if violations:
        return ScenarioResult(
            scenario_id="PRIV-SCENARIO-04",
            requirement="PRIV-001",
            description="Static audit of crates/ source tree for hardcoded telemetry domains",
            passed=False,
            details="\n".join(violations),
        )

    return ScenarioResult(
        scenario_id="PRIV-SCENARIO-04",
        requirement="PRIV-001",
        description="Static audit of crates/ source tree for hardcoded telemetry domains",
        passed=True,
        details="Scanned all Rust source files in crates/: zero tracking endpoints or domains found.",
    )


def test_log_redaction_and_data_minimization() -> ScenarioResult:
    """Verify structured log redaction, owner IP masking, and secret leak scanning."""
    passed, output = run_cargo_test("gtt-control-plane", "limiter_audit_relay_unit", "t098_audit_events_and_logs_redaction_and_data_minimization")
    return ScenarioResult(
        scenario_id="PRIV-SCENARIO-05",
        requirement="PRIV-002",
        description="Structured log redaction, owner IP masking (/24 and /48), and automated secret/token leak scanning",
        passed=passed,
        details=output if not passed else "Verified tokens, invitation secrets, and raw guest IPs are completely redacted; IPv4 masked to /24 and IPv6 to /48.",
    )


def test_retention_policy_schedules() -> ScenarioResult:
    """Verify retention policy configuration and cutoffs for all 7 data retention schedules."""
    passed, output = run_cargo_test("gtt-control-plane", "limiter_audit_relay_unit", "t109_retention_policy_config_defaults_and_cutoff_computations")
    return ScenarioResult(
        scenario_id="PRIV-SCENARIO-06",
        requirement="PRIV-003",
        description="Data retention policy schedules: guest admissions (24h), idempotency (24h), sessions (30d), tunnels (30d), usage (90d), audit (90d), deleted users (30d)",
        passed=passed,
        details=output if not passed else "Verified all 7 data retention schedules and cutoff calculations match contracts.md and requirements.md.",
    )


def test_user_account_deletion_policy() -> ScenarioResult:
    """Verify user account deletion immediate termination and cascading purge invariants."""
    passed, output = run_cargo_test("gtt-control-plane", "limiter_audit_relay_unit", "t098_account_deletion_policy_and_cascading_rules")
    return ScenarioResult(
        scenario_id="PRIV-SCENARIO-07",
        requirement="PRIV-004",
        description="Account deletion soft-delete policy, immediate resource termination, port release, session revocation, and cascading purge rules",
        passed=passed,
        details=output if not passed else "Verified soft-delete sets deleted_at, hides user, terminates active tunnels, releases ports, and revokes sessions.",
    )


def main() -> int:
    parser = argparse.ArgumentParser(description="Run Privacy Automated Test Suite (T-098)")
    parser.add_argument("--json", action="store_true", help="Output results in JSON format")
    args = parser.parse_args()

    print("================================================================================")
    print(" Network Tunneling Project -- Privacy Test Suite (T-098)")
    print(" Requirements: PRIV-001 (Zero Telemetry), PRIV-003 (Retention), PRIV-004 (Deletion)")
    print("================================================================================")
    t0 = time.perf_counter()

    scenarios = [
        test_network_capture_zero_telemetry,
        test_lifecycle_probes_strictly_scoped,
        test_dependency_register_no_telemetry_crates,
        test_codebase_zero_tracking_domains,
        test_log_redaction_and_data_minimization,
        test_retention_policy_schedules,
        test_user_account_deletion_policy,
    ]

    results: list[ScenarioResult] = []
    all_passed = True

    for runner in scenarios:
        res = runner()
        results.append(res)
        status_tag = "[PASS]" if res.passed else "[FAIL]"
        print(f"{status_tag} {res.scenario_id} ({res.requirement}): {res.description}")
        if not res.passed:
            all_passed = False
            print(f"       Details: {res.details}")

    elapsed = time.perf_counter() - t0
    print("--------------------------------------------------------------------------------")
    print(f"Summary: {sum(1 for r in results if r.passed)}/{len(results)} scenarios passed in {elapsed:.2f}s")
    print("================================================================================")

    if args.json:
        import json
        payload = [
            {
                "scenario_id": r.scenario_id,
                "requirement": r.requirement,
                "description": r.description,
                "passed": r.passed,
                "details": r.details,
            }
            for r in results
        ]
        print(json.dumps(payload, indent=2))

    return 0 if all_passed else 1


if __name__ == "__main__":
    sys.exit(main())
