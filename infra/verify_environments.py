#!/usr/bin/env python3
"""Automated Environment Isolation & Configuration Verification (T-104 / OPS-001).

Authoritative Requirement:
  - OPS-001: Development, staging, and production environments are separate.
    Acceptance Criteria: Distinct configuration, credentials, and signing keys per environment;
    production secrets are not present in staging.

Exit codes: 0 = all environment isolation checks pass; 1 = isolation defect detected.
"""

from __future__ import annotations

import os
from pathlib import Path
import re
import sys

INFRA_DIR = Path(__file__).resolve().parent
ENV_DIR = INFRA_DIR / "environments"

REQUIRED_VARS = [
    "ENVIRONMENT",
    "LOG_LEVEL",
    "CONTROL_PLANE_BIND",
    "DATABASE_URL",
    "RELAY_ID",
    "RELAY_REGION",
    "RELAY_HOST",
    "RELAY_TUNNEL_PORT",
    "RELAY_PUBLIC_PORT_START",
    "RELAY_PUBLIC_PORT_END",
    "MAX_RELAY_ALLOCATIONS",
    "TOKEN_ISSUER",
    "ACTIVE_KEY_ID",
    "SIGNING_KEY_SECRET",
    "ADMIN_RP_ID",
    "ADMIN_ORIGIN",
]


def parse_env_file(path: Path) -> dict[str, str]:
    """Parse a KEY=VALUE env file, ignoring comments and blank lines."""
    env: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if "=" in line:
            key, val = line.split("=", 1)
            env[key.strip()] = val.strip()
    return env


def main() -> int:
    print("================================================================================")
    print("          ENVIRONMENT ISOLATION & CONFIGURATION VERIFICATION (T-104)            ")
    print("================================================================================")

    dev_file = ENV_DIR / "dev.env.example"
    staging_file = ENV_DIR / "staging.env.example"
    prod_file = ENV_DIR / "production.env.example"

    for f in [dev_file, staging_file, prod_file]:
        if not f.is_file():
            print(f"[FAIL] Missing environment definition file: {f}")
            return 1

    dev_env = parse_env_file(dev_file)
    staging_env = parse_env_file(staging_file)
    prod_env = parse_env_file(prod_file)

    envs = [("development", dev_env, dev_file), ("staging", staging_env, staging_file), ("production", prod_env, prod_file)]
    failures: list[str] = []

    # 1. Required variable completeness
    print("[*] Checking required environment variable presence...")
    for env_name, env_data, fpath in envs:
        for var in REQUIRED_VARS:
            if var not in env_data or not env_data[var]:
                failures.append(f"{fpath.name}: missing required variable '{var}'")

    # 2. Distinct environment tags
    print("[*] Checking environment identities...")
    for env_name, env_data, _ in envs:
        if env_data.get("ENVIRONMENT") != env_name:
            failures.append(f"Expected ENVIRONMENT={env_name}, found {env_data.get('ENVIRONMENT')}")

    # 3. Isolation checks across environments (OPS-001)
    print("[*] Checking isolation of credentials, keys, and endpoints (OPS-001)...")
    isolation_fields = [
        ("DATABASE_URL", "Database URLs must be strictly distinct across environments"),
        ("RELAY_ID", "Relay IDs must be unique per environment"),
        ("RELAY_HOST", "Relay hostnames must be unique per environment"),
        ("TOKEN_ISSUER", "Token issuers must be unique per environment"),
        ("ACTIVE_KEY_ID", "Signing key IDs must be unique per environment"),
        ("SIGNING_KEY_SECRET", "Signing key secrets/placeholders must be unique per environment"),
        ("ADMIN_RP_ID", "Admin WebAuthn RP IDs must be unique per environment"),
        ("ADMIN_ORIGIN", "Admin origins must be unique per environment"),
    ]

    for field, msg in isolation_fields:
        vals = [dev_env.get(field), staging_env.get(field), prod_env.get(field)]
        if len(set(vals)) != 3:
            failures.append(f"Collision in {field}: {msg}. Values: {vals}")

    # 4. Production secret leak prevention (OPS-001)
    print("[*] Verifying production secrets are not present in staging or development...")
    prod_secret_val = prod_env.get("SIGNING_KEY_SECRET", "")
    prod_db_val = prod_env.get("DATABASE_URL", "")

    if prod_secret_val in [dev_env.get("SIGNING_KEY_SECRET"), staging_env.get("SIGNING_KEY_SECRET")]:
        failures.append("OPS-001 VIOLATION: Production signing key secret found in non-production config!")

    if "placeholder_prod" not in prod_secret_val:
        # Check that production example does not contain real committed secret (AGENTS.md §7)
        if not prod_secret_val.startswith("${") and "placeholder" not in prod_secret_val:
            failures.append("AGENTS.md §7 VIOLATION: Production config must use secret placeholders or vault references!")

    if "prod" in staging_env.get("DATABASE_URL", "").lower() and "staging" not in staging_env.get("DATABASE_URL", ""):
        failures.append("OPS-001 VIOLATION: Staging database URL references production database!")

    # 5. Security & TLS enforcement
    print("[*] Verifying transport security enforcement...")
    if "sslmode=verify-full" not in staging_env.get("DATABASE_URL", ""):
        failures.append("Staging DATABASE_URL must require 'sslmode=verify-full'")

    if "sslmode=verify-full" not in prod_env.get("DATABASE_URL", ""):
        failures.append("Production DATABASE_URL must require 'sslmode=verify-full'")

    if not prod_env.get("ADMIN_ORIGIN", "").startswith("https://"):
        failures.append("Production ADMIN_ORIGIN must use 'https://'")

    # Report results
    print("--------------------------------------------------------------------------------")
    if failures:
        print(">>> ENVIRONMENT ISOLATION FAILURES DETECTED <<<")
        for fail in failures:
            print(f"  [!] {fail}")
        return 1

    print("[PASS] All 3 environments defined with complete variable sets.")
    print("[PASS] Development, staging, and production are strictly isolated (OPS-001).")
    print("[PASS] Zero production credentials present in staging or development.")
    print("[PASS] Database TLS (sslmode=verify-full) and HTTPS origins enforced.")
    print(">>> ENVIRONMENT DEFINITION VERIFICATION (T-104) PASSED CLEANLY <<<")
    return 0


if __name__ == "__main__":
    sys.exit(main())
