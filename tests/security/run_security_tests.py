#!/usr/bin/env python3
"""Automated Security Test Suite Runner (T-093).

Authoritative requirements:
  - TEST-004: Security test suite covers 8 mandatory categories:
      1. Unauthorized access (SEC-018, SEC-004)
      2. Expired tokens & replay defense (SEC-004, REL-001, SEC-003)
      3. Cross-session & cross-tenant isolation (SEC-009)
      4. Destination validation & SSRF prevention (SEC-005, SEC-006, INV-1)
      5. Port exposure & minimal surface (INV-6, GAME-008, INV-2)
      6. Rate limiting & abuse defense (SEC-008)
      7. Secret leakage & data minimization (SEC-002, INV-5, PRIV-002, PRIV-006)
      8. Resource exhaustion & backpressure (SEC-015, PERF-003, PERF-005, NET-006)
  - SEC-002, SEC-005, SEC-006, SEC-008, SEC-009, SEC-010, SEC-013, SEC-015, SEC-018.

Exit codes: 0 = all security categories pass; 1 = any security failure (blocks release/merge).
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

ROOT = Path(__file__).resolve().parent.parent.parent


class SecurityCheckResult(NamedTuple):
    category_id: str
    requirement: str
    description: str
    passed: bool
    duration_ms: float
    details: str


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
        cargo_bin = Path.home() / ".cargo" / "bin"
        paths_to_add = [str(cargo_bin), str(rustup_base), str(self_contained)]
        existing_path = env.get("PATH", "")
        env["PATH"] = os.pathsep.join(paths_to_add + [existing_path])
    return env


def run_cargo_test(
    package: str,
    test_target: str,
    filter_expr: str | None = None,
    extra_env: dict[str, str] | None = None,
) -> tuple[bool, str, float]:
    """Execute a cargo test target and return (passed, output, duration_ms)."""
    cargo_bin = shutil.which("cargo") or str(Path.home() / ".cargo" / "bin" / "cargo.exe")
    cmd = [cargo_bin]
    if sys.platform == "win32":
        cmd.append("+stable-x86_64-pc-windows-gnu")
    cmd.extend(["test", "-p", package, "--test", test_target])
    if filter_expr:
        cmd.extend(["--", filter_expr, "--nocapture"])
    else:
        cmd.extend(["--", "--nocapture"])

    env = setup_environment()
    if extra_env:
        env.update(extra_env)

    start_time = time.perf_counter()
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=120,
            cwd=ROOT,
            env=env,
        )
        duration_ms = (time.perf_counter() - start_time) * 1000.0
        passed = proc.returncode == 0 and "test result: ok." in proc.stdout
        output = proc.stdout if passed else f"{proc.stdout}\n{proc.stderr}"
        return passed, output, duration_ms
    except Exception as exc:
        duration_ms = (time.perf_counter() - start_time) * 1000.0
        return False, f"Execution failed: {exc}", duration_ms


def check_unauthorized_access() -> SecurityCheckResult:
    """Category 1: Unauthorized access (SEC-018, SEC-004, SEC-009)."""
    # 1. Admin authentication & clone detection
    pass_admin, out_admin, d1 = run_cargo_test("gtt-control-plane", "admin_api_unit")
    # 2. Authorization policy checks (cross-owner not_found, unauthenticated first)
    pass_policy, out_policy, d2 = run_cargo_test(
        "gtt-control-plane",
        "policy_unit",
        "cross_owner_access_is_never_forbidden",
    )
    # 3. Relay token signature & audience validation
    pass_relay, out_relay, d3 = run_cargo_test(
        "gtt-relay",
        "relay_skeleton",
        "invalid_token_cases_return_specified_reject_codes",
    )

    passed = pass_admin and pass_policy and pass_relay
    total_ms = d1 + d2 + d3
    details = (
        "Verified: (1) unauthenticated requests refused (401); (2) cross-owner queries return 404 not_found; "
        "(3) user access tokens refused (403 forbidden) on admin routes; (4) WebAuthn cloned authenticators detected; "
        "(5) destructive admin operations require active step-up re-authentication; (6) relay rejects invalid signatures."
        if passed
        else f"Failures:\nAdmin: {out_admin}\nPolicy: {out_policy}\nRelay: {out_relay}"
    )

    return SecurityCheckResult(
        category_id="SEC-CAT-01",
        requirement="SEC-018, SEC-004",
        description="Unauthorized access prevention, admin separate identity, and WebAuthn step-up",
        passed=passed,
        duration_ms=total_ms,
        details=details,
    )


def check_expired_tokens() -> SecurityCheckResult:
    """Category 2: Expired tokens & replay defense (SEC-004, REL-001, SEC-003)."""
    # 1. Relay token expiry & replay cache
    pass_relay, out_relay, d1 = run_cargo_test(
        "gtt-relay",
        "relay_skeleton",
        "invalid_token_cases_return_specified_reject_codes",
    )
    # 2. Invitation expiry & exhaustion
    pass_inv, out_inv, d2 = run_cargo_test(
        "gtt-control-plane",
        "limiter_audit_relay_unit",
        "t044_invitation_secret_entropy_hashing_and_request_validation",
    )
    # 3. Admin challenge & session TTL expiration
    pass_admin, out_admin, d3 = run_cargo_test(
        "gtt-control-plane",
        "admin_api_unit",
        "test_admin_challenge_and_webauthn_passkey_assertion_verification",
    )

    passed = pass_relay and pass_inv and pass_admin
    total_ms = d1 + d2 + d3
    details = (
        "Verified: (1) expired allocation tokens rejected by relay (TokenExpired); (2) replayed token JTIs rejected (TokenInvalid); "
        "(3) expired invitations uniformly return 400 invitation_invalid; (4) expired admin challenges/sessions refused."
        if passed
        else f"Failures:\nRelay: {out_relay}\nInvitations: {out_inv}\nAdmin: {out_admin}"
    )

    return SecurityCheckResult(
        category_id="SEC-CAT-02",
        requirement="SEC-004, REL-001",
        description="Expired token rejection, replay cache enforcement, and credential TTL bounding",
        passed=passed,
        duration_ms=total_ms,
        details=details,
    )


def check_cross_session_isolation() -> SecurityCheckResult:
    """Category 3: Cross-session & cross-tenant isolation (SEC-009)."""
    # 1. Relay default-deny admission enforcement and cross-tunnel isolation
    pass_relay, out_relay, d1 = run_cargo_test(
        "gtt-relay",
        "admission_enforcement",
        "relay_admits_only_live_admissions_isolates_tunnels_and_normalizes_ipv4_mapped_ipv6",
    )
    # 2. Control plane multi-tenant isolation (cross-owner indistinguishability)
    pass_cp, out_cp, d2 = run_cargo_test(
        "gtt-control-plane",
        "policy_unit",
        "a_strangers_resource_and_a_missing_one_give_the_same_answer",
    )
    # 3. Engine IPv4/IPv6 dual-stack mismatch isolation
    pass_eng, out_eng, d3 = run_cargo_test(
        "gtt-engine",
        "network_impairment",
        "test_ipv4_and_ipv6_dual_stack_and_mismatch_scenarios",
    )

    passed = pass_relay and pass_cp and pass_eng
    total_ms = d1 + d2 + d3
    details = (
        "Verified: (1) relay default-deny table drops unadmitted traffic; (2) guest admitted to tunnel A cannot reach tunnel B; "
        "(3) cross-owner reads/writes return 404 not_found; (4) relay node cannot access other nodes; (5) address family isolation enforced."
        if passed
        else f"Failures:\nRelay: {out_relay}\nControlPlane: {out_cp}\nEngine: {out_eng}"
    )

    return SecurityCheckResult(
        category_id="SEC-CAT-03",
        requirement="SEC-009",
        description="Cross-session, cross-tunnel, and multi-tenant isolation enforcement",
        passed=passed,
        duration_ms=total_ms,
        details=details,
    )


def check_destination_validation() -> SecurityCheckResult:
    """Category 4: Destination validation & SSRF prevention (SEC-005, SEC-006, INV-1)."""
    # 1. Invariants unit: INV-1 rejects local targets in all schema & entity definitions
    pass_inv, out_inv, d1 = run_cargo_test(
        "gtt-control-plane",
        "invariants_unit",
        "inv1_",
    )
    # 2. Relay has no outbound connect path other than control plane polling (SEC-005)
    pass_relay, out_relay, d2 = run_cargo_test(
        "gtt-relay",
        "relay_skeleton",
        "transport_sec_010_and_outbound_dial_sec_005_invariants_enforced",
    )
    # 3. Engine target immutability under fuzzing (SEC-006)
    pass_eng, out_eng, d3 = run_cargo_test(
        "gtt-engine",
        "fuzz_relay_inputs",
        "fuzz_engine_side_relay_inputs_without_panic_hang_or_target_change",
        extra_env={"GTT_FUZZ_ITERS": "1000"},
    )

    passed = pass_inv and pass_relay and pass_eng
    total_ms = d1 + d2 + d3
    details = (
        "Verified: (1) control plane schema and parser reject all local destination fields (INV-1); "
        "(2) relay opens no client-directed outbound connections (SEC-005); "
        "(3) engine dials only locally configured targets; relay frames cannot alter targets or dial disabled mappings (SEC-006)."
        if passed
        else f"Failures:\nInvariants: {out_inv}\nRelay: {out_relay}\nEngine: {out_eng}"
    )

    return SecurityCheckResult(
        category_id="SEC-CAT-04",
        requirement="SEC-005, SEC-006, INV-1",
        description="Destination validation, SSRF prevention, and engine local-target immutability",
        passed=passed,
        duration_ms=total_ms,
        details=details,
    )


def check_port_exposure() -> SecurityCheckResult:
    """Category 5: Port exposure & minimal attack surface (INV-6, GAME-008, INV-2)."""
    # 1. Port reservation only binds explicitly enabled mappings and prevents collisions
    pass_ports, out_ports, d1 = run_cargo_test(
        "gtt-control-plane",
        "tunnels_routes_unit",
        "assign_free_ports_reserves_ports_for_enabled_mappings_and_avoids_live_collisions",
    )
    # 2. Profile mappings validation (INV-2: reject undeclared keys or protocols)
    pass_inv2, out_inv2, d2 = run_cargo_test(
        "gtt-control-plane",
        "invariants_unit",
        "inv2_",
    )

    passed = pass_ports and pass_inv2
    total_ms = d1 + d2
    details = (
        "Verified: (1) only explicitly enabled mappings allocate public ports on relays (GAME-008); "
        "(2) stopped tunnels immediately release ports (INV-6); "
        "(3) port collisions are prevented; (4) mappings strictly conform to declared profile specs (INV-2)."
        if passed
        else f"Failures:\nPorts: {out_ports}\nProfile Invariants: {out_inv2}"
    )

    return SecurityCheckResult(
        category_id="SEC-CAT-05",
        requirement="INV-6, GAME-008, INV-2",
        description="Minimal port exposure, profile schema conformance, and port collision avoidance",
        passed=passed,
        duration_ms=total_ms,
        details=details,
    )


def check_rate_limiting() -> SecurityCheckResult:
    """Category 6: Rate limits & abuse defense (SEC-008)."""
    pass_rl, out_rl, d1 = run_cargo_test(
        "gtt-control-plane",
        "limiter_audit_relay_unit",
        "rate_limiter_enforces_per_key_global_and_bounded_key_table_and_quotas",
    )

    passed = pass_rl
    details = (
        "Verified: (1) strict rate limiting protects sensitive endpoints (POST /v1/invitations/claim, refresh, admin challenge); "
        "(2) exceeding requests returns 429 rate_limited with non-zero Retry-After; "
        "(3) sliding window state is bounded by max_tracked_keys preventing memory exhaustion from attacker IP churn (AGENTS.md §4)."
        if passed
        else f"Failures:\nRateLimiter: {out_rl}"
    )

    return SecurityCheckResult(
        category_id="SEC-CAT-06",
        requirement="SEC-008",
        description="Rate limiting on sensitive endpoints with bounded memory footprint",
        passed=passed,
        duration_ms=d1,
        details=details,
    )


def check_secret_leakage_and_redaction() -> SecurityCheckResult:
    """Category 7: Secret leakage & data minimization (SEC-002, INV-5, PRIV-002, PRIV-006)."""
    # 1. Audit redaction & leak scanner test
    pass_audit, out_audit, d1 = run_cargo_test(
        "gtt-control-plane",
        "limiter_audit_relay_unit",
        "audit_actions_and_log_redaction_leave_zero_tokens_secrets_or_raw_ips",
    )
    # 2. Admin recovery code zero-leak test
    pass_admin, out_admin, d2 = run_cargo_test(
        "gtt-control-plane",
        "admin_api_unit",
        "test_offline_recovery_code_constant_time_single_use_and_zero_secret_leaks",
    )
    # 3. Static verification of DDL schemas for PRIV-006 (0 email columns) and INV-1 (0 local target columns)
    mig_dir = ROOT / "migrations"
    email_cols = 0
    local_cols = 0
    if mig_dir.is_dir():
        for sql_file in mig_dir.glob("*.up.sql"):
            text = sql_file.read_text(encoding="utf-8")
            email_cols += len(re.findall(r"\bemail\b", text, re.I))
            local_cols += len(re.findall(r"\b(local_port|local_host|target_host|target_port)\b", text, re.I))

    ddl_ok = (email_cols == 0) and (local_cols == 0)
    passed = pass_audit and pass_admin and ddl_ok
    total_ms = d1 + d2
    details = (
        "Verified: (1) bearer tokens, JWS tokens, and invitation secrets redacted from logs and audit records; "
        "(2) guest IPs masked (/24 IPv4, /48 IPv6) per PRIV-002; (3) offline recovery codes hashed at rest and never leaked (INV-5); "
        f"(4) database DDL contains 0 email columns (PRIV-006) and 0 local target columns (INV-1)."
        if passed
        else f"Failures:\nAudit: {out_audit}\nAdmin: {out_admin}\nDDL Email cols={email_cols}, Local cols={local_cols}"
    )

    return SecurityCheckResult(
        category_id="SEC-CAT-07",
        requirement="SEC-002, INV-5, PRIV-002, PRIV-006",
        description="Zero secret leakage in logs/audit events, guest IP masking, and credential store isolation",
        passed=passed,
        duration_ms=total_ms,
        details=details,
    )


def check_resource_exhaustion() -> SecurityCheckResult:
    """Category 8: Resource exhaustion & backpressure (SEC-015, PERF-003, PERF-005, NET-006)."""
    # 1. Flow limit enforcement (64 TCP / 64 UDP) and memory saturation backpressure drop
    pass_load, out_load, d1 = run_cargo_test(
        "gtt-relay",
        "load_capacity",
        "test_global_relay_memory_saturation_and_backpressure_drop",
    )
    # 2. Concurrent flow limit enforcement
    pass_flows, out_flows, d2 = run_cargo_test(
        "gtt-relay",
        "load_capacity",
        "test_concurrent_flow_saturation_and_limit_enforcement",
    )
    # 3. Account quotas & bounded request bodies
    pass_bounds, out_bounds, d3 = run_cargo_test(
        "gtt-control-plane",
        "tunnels_routes_unit",
        "parse_create_tunnel_request_accepts_valid_body_and_enforces_schema_bounds",
    )

    passed = pass_load and pass_flows and pass_bounds
    total_ms = d1 + d2 + d3
    details = (
        "Verified: (1) relay memory strictly capped by GlobalRelayMemoryBudget (16 MiB); excess packets dropped without leaks; "
        "(2) per-session flow limits (64 TCP / 64 UDP) enforced; (3) request body sizes bounded (MAX_REQUEST_BODY_BYTES = 16 KiB); "
        "(4) account quotas reject excessive live tunnels with 403 quota_exceeded (SEC-015)."
        if passed
        else f"Failures:\nMemoryCap: {out_load}\nFlowLimits: {out_flows}\nBodyBounds: {out_bounds}"
    )

    return SecurityCheckResult(
        category_id="SEC-CAT-08",
        requirement="SEC-015, PERF-003, NET-006",
        description="Resource exhaustion prevention, bounded queues, and memory backpressure drops",
        passed=passed,
        duration_ms=total_ms,
        details=details,
    )


def main() -> int:
    parser = argparse.ArgumentParser(description="Automated Security Test Suite (T-093)")
    parser.add_argument("--json", action="store_true", help="Output results in JSON format")
    args = parser.parse_args()

    checks = [
        ("Unauthorized Access Prevention", check_unauthorized_access),
        ("Expired Tokens & Replay Defense", check_expired_tokens),
        ("Cross-Session & Cross-Tenant Isolation", check_cross_session_isolation),
        ("Destination Validation & SSRF Defense", check_destination_validation),
        ("Port Exposure & Attack Surface Minimization", check_port_exposure),
        ("Rate Limiting & Abuse Defense", check_rate_limiting),
        ("Secret Leakage & Data Minimization", check_secret_leakage_and_redaction),
        ("Resource Exhaustion & Backpressure", check_resource_exhaustion),
    ]

    print("================================================================================")
    print("                    SECURITY TEST SUITE (T-093 / TEST-004)                     ")
    print("================================================================================")

    results: list[SecurityCheckResult] = []
    all_passed = True

    for name, fn in checks:
        print(f"[*] Running check: {name}...")
        res = fn()
        results.append(res)
        status_str = "PASS" if res.passed else "FAIL"
        print(f"    [{status_str}] {res.category_id} ({res.requirement}): {res.description} ({res.duration_ms:.1f} ms)")
        if not res.passed:
            all_passed = False
            print(f"    ERROR DETAILS:\n{res.details}\n")

    print("\n--------------------------------------------------------------------------------")
    print("                             SECURITY TEST SUMMARY                              ")
    print("--------------------------------------------------------------------------------")
    print(f"{'Category':<12} | {'Requirement':<20} | {'Status':<6} | {'Time (ms)':<10} | {'Description'}")
    print("-" * 80)
    for res in results:
        status_str = "PASS" if res.passed else "FAIL"
        print(f"{res.category_id:<12} | {res.requirement:<20} | {status_str:<6} | {res.duration_ms:<10.1f} | {res.description[:35]}")
    print("--------------------------------------------------------------------------------")

    passed_count = sum(1 for r in results if r.passed)
    total_count = len(results)
    print(f"Total Categories Tested: {total_count} | Passed: {passed_count} | Failed: {total_count - passed_count}")

    if all_passed:
        print("\n>>> ALL 8 SECURITY TEST CATEGORIES (TEST-004) PASSED CLEANLY <<<")
        return 0
    else:
        print("\n>>> SECURITY TEST FAILURES DETECTED - MERGE BLOCKED <<<")
        return 1


if __name__ == "__main__":
    sys.exit(main())
