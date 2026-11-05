#!/usr/bin/env python3
"""
Authentication, Security & Credential Store Verification Harness (`T-040`, `T-074`, `T-081`).

Exercises:
1. `T-040` (`SEC-001`, `SEC-003`, `FR-001`, `ADR-018`, `contracts.md` §§5.1–5.4, `api.md` ##1–4):
   - Protocol DTO validation (`AuthExchangeRequest`, `AuthRefreshRequest`, `AuthTokenResponse`, `UserProfileResponse`).
   - PKCE S256 validation (RFC 7636) and strict rejection of `code_challenge_method: "plain"` (`ADR-018`).
   - Redirect URI allow-list validation.
   - Access token issuance (HMAC-SHA256) and tampering detection.
   - High-entropy refresh token generation (`gtt_rt_...`), SHA-256 token hashing in database sessions.
   - Atomic session rotation and single-use refresh token reuse family revocation (`SEC-003`).
2. `T-074` (`SEC-002`, `PRIV-002`):
   - `SecretString` automatic memory zeroization on drop and `[REDACTED]` display/debug formatting.
   - `EncryptedFileVault` encryption/decryption roundtrip (AES-256-CTR keystream + HMAC-SHA256 tag).
   - Rejection of tampered ciphertext envelopes.
   - Automated recursive leak scan of files, logs, and dumps confirming zero plaintext secrets (`SEC-002`).
3. `T-081` (`FR-001`, `UX-001`):
   - Fresh-install onboarding flow (`S1` Welcome with verbatim encryption & privacy disclosures `PRIV-005`).
   - Browser sign-in handoff (`S2` waiting card, PKCE S256 parameters, RFC 8252 loopback callback on `127.0.0.1`).
   - Sign-in cancellation and account restriction/suspension error handling (`forbidden`).
   - Transition to signed-in state with user profile and dynamic sidebar synchronization (`FR-001`).
   - Account overview screen (`S10` with host identity, monthly relay egress quota progress, and vault security details).
   - Sign-out and session revocation, confirming that former refresh tokens are rejected with `unauthenticated` (`FR-001`).
4. Quality & Compliance:
   - Taste-skill frontend linter (`0 FAIL, 0 WARN`).
   - Strict WCAG AA contrast check (`check_contrast.py`).
   - Traceability audit (`audit_traceability.py`).
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


def run_checked(cmd: list[str], description: str, env: dict[str, str] | None = None, cwd: pathlib.Path | None = None) -> None:
    print(f"\n--- [RUNNING] {description} ---")
    print(" ".join(cmd))
    res = subprocess.run(cmd, env=env, cwd=cwd or REPO_ROOT, text=True)
    if res.returncode != 0:
        print(f"[FAILED] {description} exited with code {res.returncode}", file=sys.stderr)
        sys.exit(res.returncode)
    print(f"[OK] {description} passed.")


def main() -> None:
    print("================================================================================")
    print(" GTT Authentication, Security & Credential Store Verification (T-040, T-074, T-081)")
    print("================================================================================")

    env = build_rust_env()
    cargo_prefix = cargo_cmd_prefix(env)

    # 1. T-040: Protocol auth DTOs and validation
    run_checked(
        cargo_prefix + ["test", "-p", "gtt-protocol"],
        "T-040: gtt-protocol DTOs & PKCE validation tests",
        env=env,
    )

    # 2. T-040: Control-plane auth unit tests
    run_checked(
        cargo_prefix + ["test", "-p", "gtt-control-plane", "--test", "auth_unit"],
        "T-040: gtt-control-plane auth route unit tests (PKCE S256, access & refresh token lifecycle)",
        env=env,
    )

    # 3. T-074: Desktop credential-store integration & zero-leak scan
    run_checked(
        cargo_prefix + ["test", "-p", "gtt-desktop", "--test", "credential_store_leak_scan"],
        "T-074: gtt-desktop credential vault encryption & zero-leak scan (SEC-002, PRIV-002)",
        env=env,
    )

    npm_exe = shutil.which("npm") or ("npm.cmd" if sys.platform == "win32" else "npm")
    node_exe = shutil.which("node") or "node"

    # 4. T-081: Desktop onboarding and sign-in UI test suite
    run_checked(
        [node_exe, "apps/desktop/tests/onboarding_and_signin_test.js"],
        "T-081: Desktop onboarding, browser sign-in & sign-out UI test suite (FR-001, UX-001)",
    )

    # 5. Full Desktop UI Test Suite (all 8 suites)
    run_checked(
        [npm_exe, "test", "--prefix", "apps/desktop"],
        "T-080..T-088: Full desktop test harness (8 suites)",
    )

    # 6. Quality: Taste-skill linter (0 FAIL, 0 WARN)
    linter_path = pathlib.Path.home() / ".gemini" / "config" / "skills" / "design-taste-frontend" / "scripts" / "lint.mjs"
    if linter_path.exists():
        run_checked(
            [node_exe, str(linter_path), "--app", "apps/desktop"],
            "Quality: Design-taste frontend linter (0 FAIL, 0 WARN)",
        )
    else:
        print("[SKIP] Taste-skill linter not found at default global path.")

    # 7. Quality: Strict WCAG AA contrast check
    run_checked(
        [sys.executable, "scripts/check_contrast.py", "--strict"],
        "Quality: Strict WCAG AA contrast check (UX-005)",
    )

    # 8. Traceability Audit (0 errors)
    run_checked(
        [sys.executable, "scripts/audit_traceability.py"],
        "Traceability: Comprehensive requirements and task audit (0 errors)",
    )

    print("\n================================================================================")
    print(" ALL AUTHENTICATION, SECURITY & CREDENTIAL STORE CHECKS PASSED (EXIT 0)")
    print("================================================================================")


if __name__ == "__main__":
    main()
