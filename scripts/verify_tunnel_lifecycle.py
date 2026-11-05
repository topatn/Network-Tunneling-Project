#!/usr/bin/env python3
"""
Control-Plane Tunnel Lifecycle & Relay Allocation Verification Harness (`T-042`, `T-043`, `T-045`).

Exercises:
1. `T-042` (`FR-004`, `SEC-009`, `INV-1`, `INV-2`, `contracts.md` §§5.6–5.8, `api.md` ##6–8):
   - `CreateTunnelRequest` schema validation against `contracts/create-tunnel-request.schema.json`.
   - Strict `INV-1` local target rejection (rejecting `local_port`, `target_addr` at top-level and in mappings).
   - Idempotency-Key validation (`8..=128` printable ASCII characters) and SHA-256 canonical hashing.
   - Idempotent replay: identical request produces identical hash; modified request produces `idempotency_mismatch`.
   - Shipped profile registry loading and `INV-2` mapping validation.
   - `TunnelDto` serialization and schema validation across all lifecycle states (`created`, `active`, `stopped`, `terminated`, `expired`).
2. `T-043` (`FR-005`, `FR-006`, `REL-002`, `INV-3`, `INV-6`, `INV-7`, `contracts.md` §§5.9–5.11, `api.md` ##9–11):
   - Relay selection algorithm (`REL-002`): filters strictly to healthy relays with available capacity, skips unhealthy, draining, provisioning, retired, full, or failover-excluded relays, and orders by lowest load.
   - Port allocator: assigns free public ports in `[port_range_start, port_range_end]`, avoids live port collisions (`INV-6`), strictly ignores disabled mappings (`GAME-008`), and detects port range exhaustion.
   - Allocation token issuance and cryptographic verification (HMAC-SHA256 with key ring rotation).
   - `StartTunnelResponse` and `IssueTunnelTokenResponse` schema validation.
   - Stop tunnel state transition releasing ports and revoking invitations (`INV-7`).
3. `T-045` (`SEC-008`, `SEC-015`):
   - Sliding-window rate limiter enforcing strict, normal, and lenient rate limit windows with bounded memory eviction.
   - Account quota enforcer returning HTTP 422 `quota_exceeded` when limits are reached.
   - Atomic suspension cascades terminating tunnels, releasing allocations, and revoking sessions (`SEC-015`).
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


def run_checked(cmd: list[str], description: str, env: dict[str, str] | None = None) -> None:
    print(f"\n--- [RUNNING] {description} ---")
    print(" ".join(cmd))
    res = subprocess.run(cmd, env=env, cwd=REPO_ROOT, text=True)
    if res.returncode != 0:
        print(f"[FAILED] {description} exited with code {res.returncode}", file=sys.stderr)
        sys.exit(res.returncode)
    print(f"[OK] {description} passed.")


def main() -> None:
    print("================================================================================")
    print(" Control-Plane Tunnel Lifecycle & Relay Allocation Verification (T-042, T-043, T-045)")
    print("================================================================================")

    env = build_rust_env()
    cargo_prefix = cargo_cmd_prefix(env)

    # 1. T-042 & T-043: Control-plane tunnel routes unit test suite
    run_checked(
        cargo_prefix + ["test", "-p", "gtt-control-plane", "--test", "tunnels_routes_unit"],
        "T-042 / T-043: Tunnel create/get, idempotency, relay selection & port allocation unit tests",
        env=env,
    )

    # 2. T-045: Limiter, quotas, and audit actions unit test suite
    run_checked(
        cargo_prefix + ["test", "-p", "gtt-control-plane", "--test", "limiter_audit_relay_unit"],
        "T-045: Rate limiter, quotas, account suspension & audit unit tests",
        env=env,
    )

    # 3. Protocol framing and contract schemas
    run_checked(
        cargo_prefix + ["test", "-p", "gtt-protocol"],
        "gtt-protocol: Contract schemas & DTO validation tests",
        env=env,
    )

    # 4. Invariants unit tests (INV-1..INV-5 pure logic)
    run_checked(
        cargo_prefix + ["test", "-p", "gtt-control-plane", "--test", "invariants_unit"],
        "INV-1..INV-5: Architectural invariant unit verification",
        env=env,
    )

    # 5. Database invariants test compilation & dry-run check
    run_checked(
        cargo_prefix + ["test", "-p", "gtt-control-plane", "--test", "invariants_db"],
        "invariants_db: Database integration test binary compile and dry run",
        env=env,
    )

    # 6. Clippy warning check on control-plane
    run_checked(
        cargo_prefix + ["clippy", "-p", "gtt-control-plane", "--all-targets", "--", "-D", "warnings"],
        "Quality: gtt-control-plane clippy zero-warning check",
        env=env,
    )

    # 7. Traceability audit
    run_checked(
        [sys.executable, "scripts/audit_traceability.py"],
        "Traceability: Comprehensive requirements and task audit (0 errors)",
    )

    print("\n================================================================================")
    print(" ALL TUNNEL LIFECYCLE & RELAY ALLOCATION CHECKS PASSED (EXIT 0)")
    print("================================================================================")


if __name__ == "__main__":
    main()
