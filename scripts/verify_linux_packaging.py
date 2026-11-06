#!/usr/bin/env python3
"""Automated Verification Suite for GTT Linux Distribution Packages.

Verifies:
1. Freedesktop Desktop Entry syntax (gtt.desktop)
2. Systemd service unit syntax (gtt.service)
3. SVG icon valid XML well-formedness (gtt.svg)
4. Binary tarball packaging integrity (.tar.gz structure and permissions)
5. Debian package ar/tar structure (.deb, debian-binary, control, data)
6. AppDir bundle structure (AppRun, desktop, icon, usr/bin)
7. Checksums verification against SHA256SUMS
"""

from __future__ import annotations

import gzip
import hashlib
import io
import os
import re
import sys
import tarfile
import xml.etree.ElementTree as ET
from pathlib import Path

# Add packaging/linux to sys.path
REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT / "packaging" / "linux"))
from build_linux_dist import build_linux_packages


def test_desktop_entry_syntax(pkg_dir: Path) -> None:
    desktop_file = pkg_dir / "gtt.desktop"
    assert desktop_file.exists(), "gtt.desktop must exist"
    content = desktop_file.read_text(encoding="utf-8")
    assert "[Desktop Entry]" in content, "Must contain [Desktop Entry]"
    assert "Type=Application" in content, "Must declare Type=Application"
    assert "Name=Game Tunnel Tooling" in content, "Must have valid Name"
    assert "Exec=gtt %u" in content, "Must have valid Exec line"
    assert "Icon=gtt" in content, "Must have valid Icon line"
    assert "Categories=" in content, "Must have Categories declared"
    print("  [OK] Freedesktop desktop entry syntax verified")


def test_systemd_service_syntax(pkg_dir: Path) -> None:
    service_file = pkg_dir / "gtt.service"
    assert service_file.exists(), "gtt.service must exist"
    content = service_file.read_text(encoding="utf-8")
    assert "[Unit]" in content, "Must contain [Unit]"
    assert "[Service]" in content, "Must contain [Service]"
    assert "ExecStart=" in content, "Must specify ExecStart"
    assert "--headless" in content, "Must include --headless flag"
    assert "[Install]" in content, "Must contain [Install]"
    assert "WantedBy=default.target" in content, "Must specify WantedBy"
    print("  [OK] Systemd service unit syntax verified")


def test_svg_icon_well_formed(pkg_dir: Path) -> None:
    svg_file = pkg_dir / "gtt.svg"
    assert svg_file.exists(), "gtt.svg must exist"
    tree = ET.parse(svg_file)
    root = tree.getroot()
    assert root.tag.endswith("svg"), "Root element must be svg"
    assert "viewBox" in root.attrib, "Must declare viewBox"
    print("  [OK] SVG icon well-formedness verified")


def test_tarball_structure(tarball_path: Path, version: str) -> None:
    assert tarball_path.exists(), f"Tarball {tarball_path} must exist"
    with tarfile.open(tarball_path, "r:gz") as tar:
        names = {m.name: m for m in tar.getmembers()}
        expected_prefix = f"gtt-v{version}"
        required_paths = [
            f"{expected_prefix}/bin/gtt",
            f"{expected_prefix}/share/applications/gtt.desktop",
            f"{expected_prefix}/share/icons/hicolor/scalable/apps/gtt.svg",
            f"{expected_prefix}/systemd/gtt.service",
            f"{expected_prefix}/install.sh",
            f"{expected_prefix}/uninstall.sh",
        ]
        for rp in required_paths:
            assert rp in names, f"Tarball missing {rp}"

        # Permissions check
        assert names[f"{expected_prefix}/bin/gtt"].mode == 0o755, "Binary must be executable"
        assert names[f"{expected_prefix}/install.sh"].mode == 0o755, "install.sh must be executable"
        assert names[f"{expected_prefix}/uninstall.sh"].mode == 0o755, "uninstall.sh must be executable"
        assert names[f"{expected_prefix}/share/applications/gtt.desktop"].mode == 0o644
        assert names[f"{expected_prefix}/systemd/gtt.service"].mode == 0o644
    print("  [OK] Standalone tarball structure and permissions verified")


def test_deb_package_ar_structure(deb_path: Path, version: str) -> None:
    assert deb_path.exists(), f"Debian package {deb_path} must exist"
    data = deb_path.read_bytes()
    assert data.startswith(b"!<arch>\n"), "Must begin with ar magic"

    # Parse ar members
    idx = 8
    members: dict[str, bytes] = {}
    while idx < len(data):
        hdr = data[idx : idx + 60]
        if len(hdr) < 60:
            break
        name = hdr[0:16].decode("ascii").strip().rstrip("/")
        size = int(hdr[48:58].decode("ascii").strip())
        content_start = idx + 60
        content = data[content_start : content_start + size]
        members[name] = content
        idx = content_start + size
        if size % 2 != 0:
            idx += 1

    assert "debian-binary" in members, "Must contain debian-binary"
    assert members["debian-binary"] == b"2.0\n", "debian-binary must be 2.0"
    assert "control.tar.gz" in members, "Must contain control.tar.gz"
    assert "data.tar.gz" in members, "Must contain data.tar.gz"

    # Verify control.tar.gz
    with gzip.GzipFile(fileobj=io.BytesIO(members["control.tar.gz"])) as gz:
        with tarfile.open(fileobj=gz) as tar:
            ctrl_member = tar.extractfile("./control")
            assert ctrl_member is not None, "control.tar.gz must contain ./control"
            ctrl_text = ctrl_member.read().decode("utf-8")
            assert "Package: gtt" in ctrl_text, "Package name must be gtt"
            assert f"Version: {version}" in ctrl_text, "Version must match"
            assert "Architecture: amd64" in ctrl_text, "Architecture must be amd64"

    # Verify data.tar.gz
    with gzip.GzipFile(fileobj=io.BytesIO(members["data.tar.gz"])) as gz:
        with tarfile.open(fileobj=gz) as tar:
            data_names = {m.name: m for m in tar.getmembers()}
            for path in [
                "./usr/bin/gtt",
                "./usr/share/applications/gtt.desktop",
                "./usr/share/icons/hicolor/scalable/apps/gtt.svg",
                "./lib/systemd/system/gtt.service",
            ]:
                assert path in data_names, f"data.tar.gz missing {path}"
            assert data_names["./usr/bin/gtt"].mode == 0o755, "/usr/bin/gtt must be mode 0755"

    print("  [OK] Debian package (.deb) standard ar/tar structure verified")


def test_appdir_structure(appdir_path: Path) -> None:
    assert appdir_path.exists(), f"AppDir {appdir_path} must exist"
    assert (appdir_path / "AppRun").exists(), "AppDir must contain AppRun"
    assert (appdir_path / "gtt.desktop").exists(), "AppDir must contain gtt.desktop"
    assert (appdir_path / "gtt.svg").exists(), "AppDir must contain gtt.svg"
    assert (appdir_path / ".DirIcon").exists(), "AppDir must contain .DirIcon"
    assert (appdir_path / "usr" / "bin" / "gtt").exists(), "AppDir must contain usr/bin/gtt"
    assert (appdir_path / "usr" / "share" / "applications" / "gtt.desktop").exists()
    assert (appdir_path / "usr" / "share" / "icons" / "hicolor" / "scalable" / "apps" / "gtt.svg").exists()
    print("  [OK] AppImage AppDir directory structure verified")


def test_checksums_match(out_dir: Path) -> None:
    sums_file = out_dir / "SHA256SUMS"
    assert sums_file.exists(), "SHA256SUMS must exist"
    lines = sums_file.read_text(encoding="utf-8").strip().splitlines()
    for line in lines:
        parts = line.split()
        assert len(parts) == 2, f"Invalid checksum line: {line}"
        expected_hash, fname = parts
        target_path = out_dir / fname
        assert target_path.exists(), f"File {target_path} referenced in SHA256SUMS does not exist"
        computed = hashlib.sha256(target_path.read_bytes()).hexdigest()
        assert computed == expected_hash, f"Hash mismatch for {fname}"
    print("  [OK] SHA256SUMS integrity verified")


def main() -> None:
    print("Testing GTT Linux Distribution Packages & Metadata...")
    pkg_dir = REPO_ROOT / "packaging" / "linux"
    out_dir = REPO_ROOT / "dist" / "linux"
    version = "0.1.0"

    # Ensure packages are built
    binary_path = REPO_ROOT / "target" / "debug" / "gtt-desktop.exe"
    if not binary_path.exists():
        binary_path = REPO_ROOT / "target" / "release" / "gtt-desktop.exe"
    if not binary_path.exists():
        binary_path = REPO_ROOT / "target" / "debug" / "gtt-desktop"

    build_results = build_linux_packages(version, binary_path, pkg_dir, out_dir)

    test_desktop_entry_syntax(pkg_dir)
    test_systemd_service_syntax(pkg_dir)
    test_svg_icon_well_formed(pkg_dir)
    test_tarball_structure(build_results["tarball"], version)
    test_deb_package_ar_structure(build_results["deb"], version)
    test_appdir_structure(build_results["appdir"])
    test_checksums_match(out_dir)

    print("\nALL GTT LINUX DISTRIBUTION TESTS PASSED (EXIT 0).")


if __name__ == "__main__":
    main()
