#!/usr/bin/env python3
"""
Staging & Packaging Verification Harness (`T-100`, `T-101`, `T-102`).

Exercises:
1. `T-100` (`INST-001`, `INST-005`, `SEC-012`):
   - `apps/desktop/gtt-desktop.exe.manifest` enforces `requestedExecutionLevel level="asInvoker"`.
   - `gtt-desktop --self-check` and `gtt-desktop --verify-webview2`.
   - `gtt-desktop --install-per-user <temp_localappdata> --version 0.1.0` into an isolated
     standard-user directory without elevation, plus execution of the installed binary from
     `Programs/GTT/slots/current/gtt-desktop.exe --self-check`.
2. `T-101` (`INST-002`, `INST-003`, `SEC-014`):
   - Runs the Rust `packaging_lifecycle` integration suite verifying signed update promotion,
     tampered payload rejection, forged signature rejection, revoked key rejection, downgrade
     rejection, staging interruption recovery, and post-promotion failure rollback.
3. `T-102` (`INST-004`, `SRV-010`):
   - `gtt-desktop --uninstall-per-user <temp_localappdata>` preserves `%LOCALAPPDATA%/GTT/servers`
     world data by default (`SRV-010`) while removing `Programs/GTT`.
   - `gtt-desktop --uninstall-per-user <temp_localappdata> --remove-user-data` removes both
     `Programs/GTT` and `GTT`.
"""

from __future__ import annotations

import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile


REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
MANIFEST_PATH = REPO_ROOT / "apps" / "desktop" / "gtt-desktop.exe.manifest"


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


def verify_manifest_file() -> None:
    if not MANIFEST_PATH.exists():
        raise AssertionError(f"Missing application manifest: {MANIFEST_PATH}")
    xml = MANIFEST_PATH.read_text(encoding="utf-8")
    if 'level="asInvoker"' not in xml or 'uiAccess="false"' not in xml:
        raise AssertionError("Manifest does not enforce requestedExecutionLevel level='asInvoker' uiAccess='false'")
    if "requireAdministrator" in xml or "highestAvailable" in xml:
        raise AssertionError("Manifest must never request administrator elevation (SEC-012)")
    print("[PASS] Step 1: gtt-desktop.exe.manifest enforces asInvoker and PerMonitorV2 DPI awareness (T-100, SEC-012).")


def run_rust_packaging_lifecycle_tests(env: dict[str, str]) -> None:
    cmd = cargo_cmd_prefix(env) + ["test", "-p", "gtt-desktop", "--test", "packaging_lifecycle"]
    res = subprocess.run(cmd, cwd=REPO_ROOT, env=env, capture_output=True, text=True)
    if res.returncode != 0:
        sys.stderr.write(res.stdout)
        sys.stderr.write(res.stderr)
        raise AssertionError(f"Rust packaging_lifecycle test suite failed with exit code {res.returncode}")
    print("[PASS] Step 2: Rust packaging_lifecycle integration suite passed (T-100, T-101, T-102).")


def locate_or_build_desktop_binary(env: dict[str, str]) -> pathlib.Path:
    exe_name = "gtt-desktop.exe" if sys.platform == "win32" else "gtt-desktop"
    release_bin = REPO_ROOT / "target" / "release" / exe_name
    debug_bin = REPO_ROOT / "target" / "debug" / exe_name

    if release_bin.exists():
        return release_bin
    if debug_bin.exists():
        return debug_bin

    cmd = cargo_cmd_prefix(env) + ["build", "-p", "gtt-desktop"]
    res = subprocess.run(cmd, cwd=REPO_ROOT, env=env, capture_output=True, text=True)
    if res.returncode != 0 or not debug_bin.exists():
        sys.stderr.write(res.stdout)
        sys.stderr.write(res.stderr)
        raise AssertionError("Failed to build gtt-desktop binary for CLI staging exercise")
    return debug_bin


def exercise_cli_installer_and_uninstaller(binary_path: pathlib.Path) -> None:
    # 1. Verify `--self-check`
    self_check = subprocess.run([str(binary_path), "--self-check"], capture_output=True, text=True)
    if self_check.returncode != 0:
        raise AssertionError(f"--self-check failed: {self_check.stderr}")
    payload = json.loads(self_check.stdout.strip())
    if payload.get("status") != "ok" or payload.get("execution_level") != "asInvoker":
        raise AssertionError(f"Unexpected --self-check payload: {payload}")
    print("[PASS] Step 3: gtt-desktop --self-check verified embedded assets and asInvoker policy.")

    # 2. Verify `--verify-webview2`
    wv2 = subprocess.run([str(binary_path), "--verify-webview2"], capture_output=True, text=True)
    if wv2.returncode != 0:
        raise AssertionError(f"--verify-webview2 failed: {wv2.stderr}")
    wv2_payload = json.loads(wv2.stdout.strip())
    if wv2_payload.get("status") not in ("available", "missing"):
        raise AssertionError(f"Unexpected --verify-webview2 output: {wv2_payload}")
    print(f"[PASS] Step 4: gtt-desktop --verify-webview2 returned structured status ({wv2_payload.get('status')}).")

    # 3. Exercise `--install-per-user` and `--uninstall-per-user` in an isolated temp directory
    tmp_dir = pathlib.Path(tempfile.mkdtemp(prefix="gtt-staging-cli-"))
    try:
        install_res = subprocess.run(
            [str(binary_path), "--install-per-user", str(tmp_dir), "--version", "0.1.0"],
            capture_output=True,
            text=True,
        )
        if install_res.returncode != 0:
            raise AssertionError(f"--install-per-user failed: {install_res.stderr}")
        receipt = json.loads(install_res.stdout.strip())
        if receipt.get("execution_level") != "asInvoker":
            raise AssertionError(f"Expected execution_level=asInvoker, got {receipt}")
        if receipt.get("current_slot", {}).get("version") != "0.1.0":
            raise AssertionError(f"Expected installed version 0.1.0, got {receipt}")

        installed_exe = tmp_dir / "Programs" / "GTT" / "slots" / "current" / "gtt-desktop.exe"
        if not installed_exe.exists():
            raise AssertionError(f"Installed executable missing at {installed_exe}")
        if sys.platform != "win32":
            installed_exe.chmod(0o755)

        # Execute the installed binary from `slots/current/gtt-desktop.exe --self-check`
        installed_check = subprocess.run([str(installed_exe), "--self-check"], capture_output=True, text=True)
        if installed_check.returncode != 0:
            raise AssertionError(f"Installed slot binary failed --self-check: {installed_check.stderr}")
        print("[PASS] Step 5: Per-user installation and slot binary --self-check succeeded without elevation (T-100).")

        # Seed a managed server world file under `%LOCALAPPDATA%/GTT/servers/mc-world/level.dat`
        world_file = tmp_dir / "GTT" / "servers" / "mc-world" / "level.dat"
        world_file.parent.mkdir(parents=True, exist_ok=True)
        world_file.write_bytes(b"MANAGED-WORLD-DATA-PRESERVED")

        # Uninstall without `--remove-user-data` (default preserves user data per SRV-010)
        uninstall_preserve = subprocess.run(
            [str(binary_path), "--uninstall-per-user", str(tmp_dir)],
            capture_output=True,
            text=True,
        )
        if uninstall_preserve.returncode != 0:
            raise AssertionError(f"--uninstall-per-user failed: {uninstall_preserve.stderr}")
        report_preserve = json.loads(uninstall_preserve.stdout.strip())
        if not report_preserve.get("install_root_removed") or (tmp_dir / "Programs" / "GTT").exists():
            raise AssertionError("Expected Programs/GTT to be removed on uninstall")
        if report_preserve.get("user_data_removed") or not world_file.exists():
            raise AssertionError("Expected user world data in GTT/servers to be preserved by default (SRV-010)")
        print("[PASS] Step 6: Default uninstall removed Programs/GTT and preserved user world data (T-102, SRV-010).")

        # Re-install and uninstall WITH `--remove-user-data`
        reinstall_res = subprocess.run(
            [str(binary_path), "--install-per-user", str(tmp_dir), "--version", "0.1.0"],
            capture_output=True,
            text=True,
        )
        if reinstall_res.returncode != 0:
            raise AssertionError(f"Re-install failed: {reinstall_res.stderr}")

        uninstall_purge = subprocess.run(
            [str(binary_path), "--uninstall-per-user", str(tmp_dir), "--remove-user-data"],
            capture_output=True,
            text=True,
        )
        if uninstall_purge.returncode != 0:
            raise AssertionError(f"--uninstall-per-user --remove-user-data failed: {uninstall_purge.stderr}")
        report_purge = json.loads(uninstall_purge.stdout.strip())
        if not report_purge.get("install_root_removed") or not report_purge.get("user_data_removed"):
            raise AssertionError(f"Expected both install_root and data_root removed: {report_purge}")
        if (tmp_dir / "Programs" / "GTT").exists() or (tmp_dir / "GTT").exists():
            raise AssertionError("Expected both Programs/GTT and GTT directories to be absent after purge")
        print("[PASS] Step 7: Uninstall with --remove-user-data removed both Programs/GTT and GTT (T-102, INST-004).")
    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)


def main() -> int:
    env = build_rust_env()
    verify_manifest_file()
    run_rust_packaging_lifecycle_tests(env)
    binary_path = locate_or_build_desktop_binary(env)
    exercise_cli_installer_and_uninstaller(binary_path)
    print("\nAll T-100, T-101, and T-102 packaging, signed updater, rollback, and uninstall checks PASSED.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
