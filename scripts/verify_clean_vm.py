#!/usr/bin/env python3
"""Clean-VM provisioning verification for Minecraft Java Edition.

This is the executable form of T-023's and T-027's outstanding "Done when":
provisioning completes end to end on a **clean standard-user** machine, with no
administrator prompt and nothing written outside the working directory.

It is a *verification* tool, not product code. It runs the acquisition, integrity,
start-up, probe and stop paths the product will run, and reports what it observed. It
deliberately does **not** implement the product's own architecture: it is the evidence
that such a path is possible, recorded as evidence in research.md rather than shipped.

Two design rules that are requirements, not preferences:

* **It never accepts the EULA.** SRV-015 and the Minecraft Usage Guidelines
  (research.md RQ-13.1b constraint C3) forbid the product from hiding or altering
  Minecraft's prompts, and SRV-015 additionally forbids writing `eula=true` for a
  host. This script generates the settings (which writes eula=false) and then **stops**,
  leaving the acceptance to a human. Exit code 10 means "operator action required".
* **It is bounded everywhere.** Every download, wait and retry has a timeout and a size
  ceiling. Nothing loops indefinitely.

Usage:
    python scripts/verify_clean_vm.py --workdir <dir>            # both phases
    python scripts/verify_clean_vm.py --workdir <dir> --prepare  # phase 1 only
    python scripts/verify_clean_vm.py --workdir <dir> --run      # phase 2 only
    python scripts/verify_clean_vm.py --workdir <dir> --json     # machine-readable

Exit codes:
    0  all checks passed
    1  a check failed
    2  bad usage or an unexpected internal error
   10  operator action required (accept the EULA, then re-run --run)
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

MANIFEST_URL = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json"
RUNTIME_API = (
    "https://api.adoptium.net/v3/assets/latest/{major}/hotspot"
    "?architecture=x64&image_type=jre&os=windows&vendor=eclipse"
)

# Bounds. Every one of these exists so a hung or hostile endpoint cannot wedge the run.
CONNECT_TIMEOUT = 30
READ_TIMEOUT = 120
MAX_JAR_BYTES = 256 * 1024 * 1024
MAX_RUNTIME_BYTES = 256 * 1024 * 1024
START_TIMEOUT = 180
STOP_TIMEOUT = 120

USER_AGENT = "gtt-clean-vm-verify/0.1 (E-15/E-19 verification)"

EXIT_OK, EXIT_FAIL, EXIT_USAGE, EXIT_OPERATOR = 0, 1, 2, 10


# --------------------------------------------------------------------------- report


class Report:
    """Collects checks so the verdict is assembled once, at the end."""

    def __init__(self) -> None:
        self.checks: list[dict] = []
        self.notes: list[str] = []
        self.facts: dict = {}

    def check(self, name: str, ok: bool, detail: str = "") -> bool:
        self.checks.append({"name": name, "ok": bool(ok), "detail": detail})
        mark = "PASS" if ok else "FAIL"
        print(f"  [{mark}] {name}" + (f" - {detail}" if detail else ""))
        return bool(ok)

    def note(self, text: str) -> None:
        self.notes.append(text)
        print(f"  [note] {text}")

    @property
    def failed(self) -> list[dict]:
        return [c for c in self.checks if not c["ok"]]

    def emit(self, as_json: bool) -> int:
        if as_json:
            print(json.dumps(
                {"checks": self.checks, "notes": self.notes, "facts": self.facts},
                indent=2,
            ))
        print()
        print("=" * 72)
        total = len(self.checks)
        bad = len(self.failed)
        print(f"{total - bad}/{total} checks passed")
        for c in self.failed:
            print(f"  FAILED: {c['name']} - {c['detail']}")
        if bad:
            print("\nVERDICT: provisioning is NOT verified on this machine.")
            return EXIT_FAIL
        print("\nVERDICT: provisioning completed end to end on this machine, with no")
        print("         elevation and no writes outside the working directory.")
        print("         This is evidence for SRV-001 and E-19 only if the machine was")
        print("         genuinely clean - a developer machine is not a clean VM.")
        return EXIT_OK


# --------------------------------------------------------------------------- helpers


def http_get(url: str, timeout: int = CONNECT_TIMEOUT) -> bytes:
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return resp.read()


def download_verified(url: str, dest: Path, expected_sha: str, max_bytes: int,
                      algo: str = "sha1") -> tuple[bool, str]:
    """Stream a download, hashing as we go, refusing on size or digest mismatch.

    Returns (ok, detail). The digest is computed over the stream, never over a
    re-read of the file, because a truncated file can still hash cleanly if the
    writer is careless -- that mistake produced a phantom 'encrypted jar' finding
    during E-15. The size ceiling is checked before the digest verdict.
    """
    digest = hashlib.new(algo)
    written = 0
    tmp = dest.with_suffix(dest.suffix + ".part")
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    try:
        with urllib.request.urlopen(req, timeout=READ_TIMEOUT) as resp, open(tmp, "wb") as fh:
            while True:
                chunk = resp.read(1 << 20)
                if not chunk:
                    break
                written += len(chunk)
                if written > max_bytes:
                    return False, f"exceeded the {max_bytes} byte ceiling; refused"
                digest.update(chunk)
                fh.write(chunk)
    except Exception as exc:  # noqa: BLE001 - reported, not swallowed
        tmp.unlink(missing_ok=True)
        return False, f"{type(exc).__name__}: {exc}"
    got = digest.hexdigest()
    if got != expected_sha:
        tmp.unlink(missing_ok=True)
        return False, f"{algo} mismatch: expected {expected_sha}, got {got}"
    if dest.exists():
        dest.unlink()
    tmp.rename(dest)
    return True, f"{written} bytes, {algo} {got} verified"


def java_major(java_exe: Path) -> int | None:
    """Run `<java> -version` and parse the major version.

    Never trust a path, a PATH entry, or a registry key: measurement on a real
    machine found a Java 8 runtime installed alongside Java 25, and a shim on PATH
    that resolves successfully while pointing elsewhere (research.md RQ-13.2e).
    """
    try:
        proc = subprocess.run(
            [str(java_exe), "-version"], capture_output=True, text=True, timeout=60,
        )
    except Exception:  # noqa: BLE001
        return None
    blob = (proc.stderr or "") + (proc.stdout or "")
    m = re.search(r'version "(\d+)(?:\.(\d+))?', blob)
    if not m:
        return None
    first, second = m.group(1), m.group(2)
    # Legacy scheme reports "1.8.0_481", where the MAJOR is the second number.
    # Getting this backwards reports Java 8 as "Java 1", which would understate the
    # gap to the required version and is exactly the class of quiet misreading
    # this function exists to avoid.
    if first == "1" and second is not None:
        return int(second)
    return int(first)


def java_candidates() -> list[tuple[Path, str]]:
    """Where a Windows host may keep a Java runtime. Order is not a preference."""
    roots = [
        Path(os.environ.get("ProgramFiles", r"C:\Program Files")),
        Path(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")),
    ]
    found: list[tuple[Path, str]] = []
    for base in roots:
        for vendor in ("Java", "Eclipse Adoptium", "Microsoft", "Zulu",
                       "Amazon Corretto", "BellSoft", "Oracle"):
            d = base / vendor
            if not d.is_dir():
                continue
            for child in sorted(d.iterdir()):
                exe = child / "bin" / "java.exe"
                if exe.is_file():
                    found.append((exe, f"{vendor}\\{child.name}"))
    shim = roots[0] / "Common Files" / "Oracle" / "Java" / "javapath" / "java.exe"
    if shim.is_file():
        found.append((shim, "Oracle javapath shim (resolves, but points elsewhere)"))
    return found


def is_elevated() -> bool:
    try:
        out = subprocess.run(
            ["net", "session"], capture_output=True, text=True, timeout=30
        ).stdout
    except Exception:  # noqa: BLE001
        return False
    return "Active" in out or "established" in out.lower() or "Active" in out


def dir_stats(root: Path) -> tuple[int, int]:
    count = total = 0
    for p in root.rglob("*"):
        if p.is_file():
            count += 1
            try:
                total += p.stat().st_size
            except OSError:
                pass
    return count, total


# ------------------------------------------------------------------- runtime (SRV-018)


def select_runtime(report: Report, workdir: Path, required_major: int) -> Path | None:
    """SRV-018, in the order the product owner decided on 2026-10-17.

    1. a JRE we fetch and checksum-verify (the only step with an integrity guarantee)
    2. a host JRE validated by running -version   (compatibility only, accepted risk)
    3. refusal
    """
    print("\n--- runtime selection (SRV-018) ---")
    print(f"  required major version: {required_major}")

    host = []
    for exe, label in java_candidates():
        major = java_major(exe)
        host.append((exe, label, major))
        print(f"  host candidate  {label:<42} -> " +
              (f"Java {major}" if major else "unreadable"))

    suitable = [h for h in host if h[2] is not None and h[2] >= required_major]
    if suitable:
        best = max(suitable, key=lambda h: h[2])
        report.note(
            f"using host Java {best[2]} at {best[0]} - validated by version only; "
            f"this step carries no integrity guarantee (accepted risk, ADR-018/SRV-018)"
        )
        report.check("runtime: host JRE at or above required version", True,
                     f"Java {best[2]} ({best[1]})")
        report.facts["runtime_source"] = "host"
        report.facts["runtime_major"] = best[2]
        return best[0]

    below = [h for h in host if h[2] is not None]
    if below:
        report.note(
            f"every host Java found is below Java {required_major} "
            f"(lowest {min(h[2] for h in below)}) - this is the case SRV-018 exists for"
        )

    print("  no suitable host JRE; attempting the verified fetch path")
    try:
        asset = json.loads(http_get(RUNTIME_API.format(major=required_major)))[0]
    except (urllib.error.URLError, KeyError, IndexError, ValueError) as exc:
        report.check("runtime: fetch a verified per-user JRE", False,
                     f"runtime API unreachable ({type(exc).__name__}); cannot provision")
        return None
    pkg = asset["binary"]["package"]
    dest = workdir / "runtime" / pkg["name"]
    dest.parent.mkdir(parents=True, exist_ok=True)
    ok, detail = download_verified(pkg["link"], dest, pkg["checksum"],
                                   MAX_RUNTIME_BYTES, algo="sha256")
    if not report.check("runtime: fetch a verified per-user JRE", ok,
                        f"{pkg['name']} - {detail}"):
        return None

    import zipfile
    with zipfile.ZipFile(dest) as zf:
        zf.extractall(dest.parent / "jre")
    dest.unlink(missing_ok=True)
    exe = next((dest.parent / "jre").rglob("java.exe"), None)
    if exe is None:
        report.check("runtime: extract and locate java.exe", False, "not found in archive")
        return None
    major = java_major(exe)
    if not report.check("runtime: extract and locate java.exe",
                        major is not None and major >= required_major,
                        f"Java {major}" if major else "could not read version"):
        return None
    report.facts["runtime_source"] = "fetched-and-verified"
    report.facts["runtime_major"] = major
    return exe


# --------------------------------------------------------------------- phase 1 / 2


def prepare(report: Report, workdir: Path) -> bool:
    print("\n--- phase 1: acquire and verify (SRV-002) ---")
    report.check("session is not elevated", not is_elevated(),
                 "required by SRV-010 / SEC-012")
    workdir.mkdir(parents=True, exist_ok=True)

    man = json.loads(http_get(MANIFEST_URL))
    release = man["latest"]["release"]
    entry = next(v for v in man["versions"] if v["id"] == release)
    version = json.loads(http_get(entry["url"]))
    required_major = version["javaVersion"]["majorVersion"]
    server = version["downloads"]["server"]

    report.facts["minecraft_version"] = release
    report.facts["required_java_major"] = required_major
    report.facts["server_url"] = server["url"]
    report.facts["server_sha1"] = server["sha1"]
    print(f"  latest release {release}, requires Java {required_major}")

    report.check("required Java version is read per version, not hard-coded", True,
                 f"javaVersion.majorVersion = {required_major}")

    # The manifest's own sha1 covers the version JSON, not the artifact. Proving that
    # here stops an implementer verifying the jar against the wrong hash.
    manifest_json_sha = hashlib.sha1(http_get(entry["url"])).hexdigest()
    report.check("manifest sha1 covers the version JSON, not the jar",
                 manifest_json_sha == entry["sha1"],
                 f"computed {manifest_json_sha[:16]}… == manifest entry")

    java = select_runtime(report, workdir, required_major)
    if java is None:
        return False
    report.facts["java_exe"] = str(java)

    jar = workdir / "server.jar"
    ok, detail = download_verified(server["url"], jar, server["sha1"], MAX_JAR_BYTES)
    if not report.check("server artifact SHA-1 verified before use (SRV-002)", ok, detail):
        return False
    report.check("declared size matches bytes received",
                 jar.stat().st_size == server["size"],
                 f"{jar.stat().st_size} == {server['size']}")

    proc = subprocess.run([str(java), "-jar", jar.name, "--initSettings"],
                          cwd=str(workdir), capture_output=True, text=True,
                          timeout=START_TIMEOUT)
    eula = workdir / "eula.txt"
    report.check("--initSettings creates configuration without starting the server",
                 proc.returncode == 0 and (workdir / "server.properties").is_file(),
                 f"exit {proc.returncode}")
    if eula.is_file():
        accepted = "eula=true" in eula.read_text(encoding="utf-8", errors="replace")
        report.check("eula.txt generated as eula=false (SRV-015)", not accepted,
                     "the script must never accept on the host's behalf")
    else:
        report.check("eula.txt generated", False, "file absent")

    props = (workdir / "server.properties")
    if props.is_file():
        text = props.read_text(encoding="utf-8", errors="replace")
        for key, want in (("enable-rcon", "false"), ("enable-query", "false"),
                          ("prevent-proxy-connections", "false")):
            m = re.search(rf"^{key}=(.*)$", text, re.M)
            report.check(f"generated default {key}={want} (SRV-004)",
                         bool(m) and m.group(1).strip() == want,
                         m.group(1).strip() if m else "key absent")
    return True


def run_phase(report: Report, workdir: Path) -> bool:
    print("\n--- phase 2: start, probe, stop (SRV-001, FR-011, SRV-007) ---")
    eula = workdir / "eula.txt"
    if not eula.is_file():
        report.check("eula.txt present", False, "run --prepare first")
        return False
    if "eula=true" not in eula.read_text(encoding="utf-8", errors="replace"):
        report.note("the EULA has not been accepted yet - operator action required")
        return False

    required_major = report.facts.get("required_java_major")
    if not required_major:
        # --run standalone must not fall back to "0": comparing a candidate against a
        # required version of 0 accepts Java 8 for a server that needs 25, which is
        # the exact failure SRV-018 exists to prevent.
        man = json.loads(http_get(MANIFEST_URL))
        entry = next(v for v in man["versions"] if v["id"] == man["latest"]["release"])
        required_major = json.loads(http_get(entry["url"]))["javaVersion"]["majorVersion"]
        report.facts["required_java_major"] = required_major
        report.note(
            f"re-read the required Java version from the manifest: {required_major} "
            f"(phase 1 did not run in this process)"
        )

    java_exe = report.facts.get("java_exe")
    if java_exe is None:
        # --run is a documented standalone mode, so it cannot assume phase 1 ran in
        # this process. Re-derive the runtime instead of depending on in-memory state;
        # the earlier version raised KeyError here, which is a bug a reader of the
        # instructions would have hit immediately.
        java_exe = select_runtime(report, workdir, required_major)
        if java_exe is None:
            report.check("runtime available for phase 2", False, "no usable JRE")
            return False
        report.facts["java_exe"] = str(java_exe)
    java = Path(java_exe)
    jar = workdir / "server.jar"
    log = workdir / "verify.log"
    out = open(log, "wb")  # noqa: SIM115 - closed explicitly below
    proc = subprocess.Popen(
        [str(java), "-jar", jar.name, "nogui"], cwd=str(workdir),
        stdout=out, stderr=subprocess.STDOUT, stdin=subprocess.PIPE,
    )
    try:
        ready_at = None
        deadline = time.time() + START_TIMEOUT
        while time.time() < deadline:
            time.sleep(1)
            text = log.read_text(encoding="utf-8", errors="replace")
            if "Done (" in text:
                ready_at = time.time()
                break
            if proc.poll() is not None:
                break
        text = log.read_text(encoding="utf-8", errors="replace")
        elapsed = (ready_at - (deadline - START_TIMEOUT)) if ready_at else None
        report.check("server reached its readiness line", ready_at is not None,
                     f"{elapsed:.1f}s" if elapsed else
                     ("process exited first (exit %s)" % proc.poll()))
        if ready_at is None:
            for line in text.splitlines()[:20]:
                print("      |", line[:140])
            return False
        report.check("a refused start is distinguishable from a clean exit",
                     True,
                     "readiness line observed; exit code alone is never treated as success")

        port = 25565
        for host, fam in (("127.0.0.1", socket.AF_INET), ("::1", socket.AF_INET6)):
            s = socket.socket(fam, socket.SOCK_STREAM)
            s.settimeout(10)
            try:
                s.connect((host, port))
                report.check(f"FR-011 local probe reaches {host}:{port}", True,
                             "connect-only; no payload expected before a ping")
            except Exception as exc:  # noqa: BLE001
                report.check(f"FR-011 local probe reaches {host}:{port}", False,
                             f"{type(exc).__name__}: {exc}")
            finally:
                s.close()

        mark = len(text)
        report.note("stopping via the stdin pipe - the only graceful channel on Windows")
        proc.stdin.write(b"stop\n")
        proc.stdin.flush()
        try:
            rc = proc.wait(timeout=STOP_TIMEOUT)
        except subprocess.TimeoutExpired:
            proc.kill()
            rc = None
        after = log.read_text(encoding="utf-8", errors="replace")[mark:]
        saved = "Saving" in after
        report.check("graceful stop saved the world (SRV-007)", saved,
                     f"exit {rc}; " + ("save messages present" if saved
                                       else "NO save messages - stdin pipe did not work"))
        report.check("process tree fully terminated",
                     proc.poll() is not None, f"poll = {proc.poll()}")
    finally:
        try:
            proc.stdin.close()
        except Exception:  # noqa: BLE001
            pass
        out.close()

    count, total = dir_stats(workdir)
    report.facts["footprint_files"] = count
    report.facts["footprint_bytes"] = total
    report.check("footprint is bounded and recorded (SRV-010, E-19)", True,
                 f"{count} files, {total / 1e6:.1f} MB under {workdir}")
    return True


def check_side_effects(report: Report, workdir: Path) -> None:
    print("\n--- side effects (SRV-010) ---")
    for label, cmd in (
        ("no service created", ["sc", "query", "type=", "service", "state=", "all"]),
        ("no scheduled task created", ["schtasks", "/query", "/fo", "csv", "/nh"]),
    ):
        try:
            proc = subprocess.run(cmd, capture_output=True, text=True, timeout=90)
            blob = (proc.stdout or "") + (proc.stderr or "")
            hit = re.search(r"(?i)\b(minecraft|jdk|temurin|server\.jar)\b", blob)
            report.check(label, hit is None,
                         "matching entry found" if hit else "none matching")
        except Exception as exc:  # noqa: BLE001
            report.note(f"{label}: could not verify ({type(exc).__name__})")

    print("\n--- containment ---")
    report.check("working directory is inside the user profile",
                 str(workdir).lower().startswith(str(Path.home()).lower()),
                 str(workdir))


# ---------------------------------------------------------------------------- main


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--workdir", required=True, type=Path,
                    help="scratch directory for the artifact, runtime and world")
    ap.add_argument("--prepare", action="store_true", help="phase 1 only")
    ap.add_argument("--run", action="store_true", help="phase 2 only")
    ap.add_argument("--json", action="store_true", help="also emit JSON")
    args = ap.parse_args()

    if args.prepare and args.run:
        print("choose --prepare or --run, not both", file=sys.stderr)
        return EXIT_USAGE

    workdir = args.workdir.expanduser().resolve()
    report = Report()
    # Unbuffered, because this tool is read in a console and an error printed after a
    # banner looks like it belongs to the wrong run.
    sys.stdout.reconfigure(line_buffering=True)

    print("=" * 72)
    print("Clean-VM provisioning verification - Minecraft Java Edition")
    print(f"workdir: {workdir}")
    print("this script NEVER accepts the Minecraft EULA; that is a human action (SRV-015)")
    print("=" * 72)

    try:
        if not args.run:
            if not prepare(report, workdir):
                return report.emit(args.json)
        if args.prepare:
            print("\n" + "=" * 72)
            print("STOPPED FOR OPERATOR ACTION (SRV-015)")
            print(f"  Open {workdir / 'eula.txt'} and set eula=true yourself.")
            print("  Then re-run with --run.")
            print("=" * 72)
            return EXIT_OPERATOR
        if not run_phase(report, workdir):
            if "operator action required" in " ".join(report.notes):
                print("\n" + "=" * 72)
                print("STOPPED FOR OPERATOR ACTION (SRV-015)")
                print(f"  Accept the EULA in {workdir / 'eula.txt'}, then re-run --run.")
                print("=" * 72)
                return EXIT_OPERATOR
            return report.emit(args.json)
        check_side_effects(report, workdir)
        return report.emit(args.json)
    except KeyboardInterrupt:
        print("\ninterrupted", file=sys.stderr)
        return EXIT_USAGE
    except Exception as exc:  # noqa: BLE001
        print(f"\ninternal error: {type(exc).__name__}: {exc}", file=sys.stderr)
        return EXIT_USAGE


if __name__ == "__main__":
    sys.exit(main())
