#!/usr/bin/env python3
"""
Comprehensive automated verification script for Security, Privacy & Protocol Fuzzing
(T-093, T-094, T-098, T-074, TEST-004, NET-007, PRIV-001..PRIV-006, SEC-001..SEC-018, INV-1..INV-7).

Runs:
1. Automated Security Test Suite (tests/security/run_security_tests.py) across 8 categories:
   - SEC-CAT-01: Unauthorized access prevention, admin separate identity, WebAuthn clone detection
   - SEC-CAT-02: Expired tokens, replay defense, credential TTL bounding
   - SEC-CAT-03: Cross-session, cross-tunnel, and multi-tenant isolation
   - SEC-CAT-04: Destination validation, SSRF prevention, engine local target immutability
   - SEC-CAT-05: Port exposure, profile schema conformance, collision avoidance
   - SEC-CAT-06: Rate limiting on sensitive endpoints with bounded memory footprint
   - SEC-CAT-07: Secret leakage, guest IP masking, credential store isolation
   - SEC-CAT-08: Resource exhaustion, queue bounding, backpressure drops
2. Privacy & Zero-Telemetry Test Suite (tests/privacy/run_privacy_tests.py):
   - Outbound network capture (zero telemetry, zero tracking domains)
   - Dependency static analysis (zero telemetry SDKs in Cargo.lock)
   - Retention schedule sweep across 7 categories
   - Account deletion immediate termination and cascading purge
3. Protocol Fuzzing Suite (T-094, NET-007):
   - Relay protocol input fuzzing (crates/relay/tests/fuzz_protocol_inputs.rs)
   - Engine relay input fuzzing (crates/engine/tests/fuzz_relay_inputs.rs)
4. Desktop Credential Vault & Leak Scan (T-074, PRIV-002, PRIV-006):
   - Ciphertext tampering detection, roundtrip redaction, and file leak scan
5. Quality: Clippy zero-warning check across workspace crates.
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
    print(" Security, Privacy & Invariant Verification (T-093, T-094, T-098, T-074)")
    print("================================================================================")

    env = build_rust_env()
    cargo = cargo_cmd_prefix(env)

    # 1. Automated Security Suite (T-093, TEST-004: 8 categories)
    run_step(
        [sys.executable, "tests/security/run_security_tests.py"],
        "T-093: Automated Security Suite (8 mandatory categories: SEC-CAT-01..SEC-CAT-08)",
        env=env,
    )

    # 2. Automated Privacy Suite (T-098: zero telemetry, retention, deletion)
    run_step(
        [sys.executable, "tests/privacy/run_privacy_tests.py"],
        "T-098: Automated Privacy & Zero-Telemetry Suite (PRIV-001, PRIV-003, PRIV-004)",
        env=env,
    )

    # 3. Protocol Input Fuzzing (T-094, NET-007)
    run_step(
        cargo + ["test", "-p", "gtt-relay", "--test", "fuzz_protocol_inputs", "--", "--nocapture"],
        "T-094: Relay protocol input fuzzing (20k iterations: control frames, TCP, UDP)",
        env=env,
    )
    run_step(
        cargo + ["test", "-p", "gtt-engine", "--test", "fuzz_relay_inputs", "--", "--nocapture"],
        "T-094: Engine relay input fuzzing (20k iterations: target immutability assert)",
        env=env,
    )

    # 4. Desktop Credential Vault & Leak Scan (T-074, PRIV-002, PRIV-006)
    run_step(
        cargo + ["test", "-p", "gtt-desktop", "--test", "credential_store_leak_scan"],
        "T-074: Desktop credential encryption, tampering detection, and zero-leak scan",
        env=env,
    )

    # 5. Quality: Clippy check on security-relevant crates
    run_step(
        cargo + [
            "clippy",
            "-p", "gtt-control-plane",
            "-p", "gtt-relay",
            "-p", "gtt-engine",
            "-p", "gtt-desktop",
            "--all-targets",
            "--",
            "-D", "warnings",
        ],
        "Quality: Zero-warning Clippy check on control-plane, relay, engine, and desktop",
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
    print(" ALL SECURITY, PRIVACY & INVARIANT CHECKS PASSED (EXIT 0)")
    print("================================================================================")


if __name__ == "__main__":
    main()
