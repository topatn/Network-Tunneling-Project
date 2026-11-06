#!/usr/bin/env python3
"""Game Tunnel Tooling (GTT) Linux Distribution Packaging Tool.

Generates Linux distribution packages from the compiled binary and metadata:
1. Standalone Binary Tarball (gtt-v{version}-linux-x86_64.tar.gz)
2. Debian Package (.deb: gtt_{version}_amd64.deb)
3. AppImage Bundle Directory (gtt.AppDir/)
4. SHA256 Checksums file (SHA256SUMS)

Uses Python standard library only; runs cross-platform on Windows, macOS, and Linux.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import os
import shutil
import stat
import sys
import tarfile
from pathlib import Path


def create_ar_archive(members: list[tuple[str, bytes]]) -> bytes:
    """Pack members into a canonical GNU ar format archive (.deb)."""
    out = io.BytesIO()
    out.write(b"!<arch>\n")
    for name, data in members:
        # File name (16 bytes, ASCII, ending in / for GNU format)
        fname = (name + "/").encode("ascii")
        if len(fname) > 16:
            raise ValueError(f"File name too long for ar archive: {name}")
        fname_hdr = fname.ljust(16, b" ")

        # mtime (12 bytes), uid (6 bytes), gid (6 bytes), mode (8 bytes), size (10 bytes), trailer (2 bytes)
        mtime = b"0           "
        uid = b"0     "
        gid = b"0     "
        mode = b"100644  "
        size = str(len(data)).encode("ascii").ljust(10, b" ")
        fmag = b"`\n"

        out.write(fname_hdr + mtime + uid + gid + mode + size + fmag)
        out.write(data)
        if len(data) % 2 != 0:
            out.write(b"\n")
    return out.getvalue()


def build_tar_gz_bytes(entries: list[tuple[str, bytes, int]]) -> bytes:
    """Create in-memory .tar.gz with (path, bytes, mode)."""
    buf = io.BytesIO()
    with gzip.GzipFile(fileobj=buf, mode="wb", mtime=0) as gz:
        with tarfile.open(fileobj=gz, mode="w") as tar:
            for path, data, mode in entries:
                ti = tarfile.TarInfo(name=path)
                ti.size = len(data)
                ti.mtime = 0
                ti.mode = mode
                ti.type = tarfile.REGTYPE
                ti.uid = 0
                ti.gid = 0
                ti.uname = "root"
                ti.gname = "root"
                tar.addfile(ti, io.BytesIO(data))
    return buf.getvalue()


def build_linux_packages(
    version: str,
    binary_path: Path,
    pkg_dir: Path,
    out_dir: Path,
) -> dict[str, Path]:
    """Build all Linux distribution packages and return their paths."""
    out_dir.mkdir(parents=True, exist_ok=True)
    results: dict[str, Path] = {}

    bin_data = binary_path.read_bytes()
    desktop_data = (pkg_dir / "gtt.desktop").read_bytes()
    service_data = (pkg_dir / "gtt.service").read_bytes()
    svg_data = (pkg_dir / "gtt.svg").read_bytes()
    apprun_data = (pkg_dir / "AppRun").read_bytes()
    install_sh_data = (pkg_dir / "install.sh").read_bytes()
    uninstall_sh_data = (pkg_dir / "uninstall.sh").read_bytes()

    # ---------------------------------------------------------
    # 1. Standalone Binary Tarball: gtt-v{version}-linux-x86_64.tar.gz
    # ---------------------------------------------------------
    tarball_name = f"gtt-v{version}-linux-x86_64.tar.gz"
    tarball_path = out_dir / tarball_name
    print(f"==> Generating standalone binary tarball: {tarball_name}...")

    tar_entries = [
        (f"gtt-v{version}/bin/gtt", bin_data, 0o755),
        (f"gtt-v{version}/share/applications/gtt.desktop", desktop_data, 0o644),
        (f"gtt-v{version}/share/icons/hicolor/scalable/apps/gtt.svg", svg_data, 0o644),
        (f"gtt-v{version}/systemd/gtt.service", service_data, 0o644),
        (f"gtt-v{version}/install.sh", install_sh_data, 0o755),
        (f"gtt-v{version}/uninstall.sh", uninstall_sh_data, 0o755),
    ]
    tarball_bytes = build_tar_gz_bytes(tar_entries)
    tarball_path.write_bytes(tarball_bytes)
    results["tarball"] = tarball_path

    # ---------------------------------------------------------
    # 2. Debian Package: gtt_{version}_amd64.deb
    # ---------------------------------------------------------
    deb_name = f"gtt_{version}_amd64.deb"
    deb_path = out_dir / deb_name
    print(f"==> Generating Debian package: {deb_name}...")

    control_content = f"""Package: gtt
Version: {version}
Section: net
Priority: optional
Architecture: amd64
Maintainer: GTT Team <support@gtt.gg>
Description: Game Tunnel Tooling (GTT)
 Host multiplayer game servers on your PC and share with friends
 over the internet without router port forwarding or public IPs.
"""
    control_tar = build_tar_gz_bytes([
        ("./control", control_content.encode("utf-8"), 0o644),
    ])

    data_tar = build_tar_gz_bytes([
        ("./usr/bin/gtt", bin_data, 0o755),
        ("./usr/share/applications/gtt.desktop", desktop_data, 0o644),
        ("./usr/share/icons/hicolor/scalable/apps/gtt.svg", svg_data, 0o644),
        ("./lib/systemd/system/gtt.service", service_data, 0o644),
    ])

    deb_bytes = create_ar_archive([
        ("debian-binary", b"2.0\n"),
        ("control.tar.gz", control_tar),
        ("data.tar.gz", data_tar),
    ])
    deb_path.write_bytes(deb_bytes)
    results["deb"] = deb_path

    # ---------------------------------------------------------
    # 3. AppImage Bundle Directory (gtt.AppDir/)
    # ---------------------------------------------------------
    appdir_path = out_dir / "gtt.AppDir"
    print(f"==> Generating AppDir layout: {appdir_path.name}...")
    if appdir_path.exists():
        shutil.rmtree(appdir_path)

    (appdir_path / "usr" / "bin").mkdir(parents=True)
    (appdir_path / "usr" / "share" / "applications").mkdir(parents=True)
    (appdir_path / "usr" / "share" / "icons" / "hicolor" / "scalable" / "apps").mkdir(parents=True)

    (appdir_path / "AppRun").write_bytes(apprun_data)
    (appdir_path / "AppRun").chmod(0o755)
    (appdir_path / "gtt.desktop").write_bytes(desktop_data)
    (appdir_path / "gtt.svg").write_bytes(svg_data)
    (appdir_path / ".DirIcon").write_bytes(svg_data)

    (appdir_path / "usr" / "bin" / "gtt").write_bytes(bin_data)
    (appdir_path / "usr" / "bin" / "gtt").chmod(0o755)
    (appdir_path / "usr" / "share" / "applications" / "gtt.desktop").write_bytes(desktop_data)
    (appdir_path / "usr" / "share" / "icons" / "hicolor" / "scalable" / "apps" / "gtt.svg").write_bytes(svg_data)

    results["appdir"] = appdir_path

    # ---------------------------------------------------------
    # 4. Compute Checksums: SHA256SUMS
    # ---------------------------------------------------------
    sums_file = out_dir / "SHA256SUMS"
    lines = []
    for p in [tarball_path, deb_path]:
        h = hashlib.sha256(p.read_bytes()).hexdigest()
        lines.append(f"{h}  {p.name}\n")
    sums_file.write_text("".join(lines), encoding="utf-8")
    results["checksums"] = sums_file

    print(f"==> Linux packaging complete in {out_dir}")
    return results


def main() -> None:
    parser = argparse.ArgumentParser(description="Package GTT for Linux")
    parser.add_argument("--version", default="0.1.0", help="Release version string")
    parser.add_argument("--binary", default="target/debug/gtt-desktop.exe", help="Binary path to package")
    parser.add_argument("--out-dir", default="dist/linux", help="Output directory")
    args = parser.parse_args()

    repo_root = Path(__file__).resolve().parent.parent.parent
    binary_path = repo_root / args.binary
    if not binary_path.exists():
        # Fallback to linux binary name or dummy build
        alt_binary = repo_root / "target" / "release" / "gtt-desktop"
        if alt_binary.exists():
            binary_path = alt_binary
        else:
            alt_debug = repo_root / "target" / "debug" / "gtt-desktop"
            if alt_debug.exists():
                binary_path = alt_debug

    if not binary_path.exists():
        print(f"Error: binary not found at {binary_path}", file=sys.stderr)
        sys.exit(1)

    pkg_dir = repo_root / "packaging" / "linux"
    out_dir = repo_root / args.out_dir
    build_linux_packages(args.version, binary_path, pkg_dir, out_dir)


if __name__ == "__main__":
    main()
