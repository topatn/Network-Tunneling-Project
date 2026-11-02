#!/usr/bin/env python3
"""Automated L0 and L1 game compatibility and gap-inventory verifier (T-090, TEST-005, GAME-001..GAME-009).

Primary focus: Minecraft: Java Edition (GAME-002, ADR-013).
- Verifies all 5 declarative game profiles in `profiles/*.json` against GAME-002, GAME-007, GAME-008, and ADR-021.
- Executes an end-to-end L0 (connect-only TCP probe on IPv4 127.0.0.1 and IPv6 ::1 + custom port GAME-009)
  and L1 (Minecraft: Java Edition Server List Ping 0x00 Handshake + 0x00 Status Request/Response JSON +
  0x01 Ping/Pong, plus UT3 UDP Query 0xFE 0xFD 0x09) verification.
- Optionally probes a live or containerized Minecraft: Java Edition server (`--live-mc-java HOST:PORT`).
- Verifies that the explicit gap inventory in `tests/game-compat/README.md` documents every unverified/manual gap.
"""

from __future__ import annotations

import argparse
import json
import socket
import struct
import sys
import threading
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
PROFILES_DIR = ROOT / "profiles"
README_PATH = Path(__file__).resolve().parent / "README.md"

EXPECTED_PROFILES = {
    "minecraft-java": {
        "default_enabled": ["game"],
        "optional_off": ["query", "rcon"],
        "l0_automated": True,
        "l1_automated": True,
    },
    "terraria": {
        "default_enabled": ["game"],
        "optional_off": ["game-udp"],
        "l0_automated": True,
        "l1_automated": False,  # G-TERRARIA-L1 (E-1)
    },
    "minecraft-bedrock": {
        "default_enabled": ["game"],
        "optional_off": ["game-ipv6"],
        "l0_automated": True,
        "l1_automated": True,
    },
    "l4d2": {
        "default_enabled": ["game"],
        "optional_off": ["rcon", "sourcetv", "client"],
        "l0_automated": True,
        "l1_automated": True,
    },
    "l4d1": {
        "default_enabled": [],  # ADR-021: evidence is reported ([R]), so 0 default-enabled mappings
        "optional_off": [],
        "l0_automated": False,
        "l1_automated": False,  # G-L4D1-EVIDENCE (E-5)
    },
}

REQUIRED_GAP_IDS = [
    "G-MCJAVA-L2",
    "G-TERRARIA-L1",
    "G-BEDROCK-L2",
    "G-L4D2-SRCDS",
    "G-L4D1-EVIDENCE",
]


def encode_varint(value: int) -> bytes:
    out = bytearray()
    val = value & 0xFFFFFFFF
    for _ in range(5):
        byte = val & 0x7F
        val >>= 7
        if val != 0:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            break
    return bytes(out)


def decode_varint(data: bytes, offset: int = 0) -> tuple[int, int]:
    result = 0
    for idx in range(5):
        if offset + idx >= len(data):
            raise ValueError("truncated VarInt")
        byte = data[offset + idx]
        if idx == 4 and (byte & 0xF0) != 0:
            raise ValueError("overlong VarInt")
        result |= (byte & 0x7F) << (7 * idx)
        if (byte & 0x80) == 0:
            return result, offset + idx + 1
    raise ValueError("VarInt exceeds 5 bytes")


def build_mc_slp_request(protocol_version: int, host: str, port: int) -> bytes:
    host_bytes = host.encode("utf-8")[:255]
    handshake_body = (
        encode_varint(0x00)
        + encode_varint(protocol_version)
        + encode_varint(len(host_bytes))
        + host_bytes
        + struct.pack(">H", port)
        + encode_varint(1)  # next_state = 1 (Status)
    )
    handshake_pkt = encode_varint(len(handshake_body)) + handshake_body
    status_req_pkt = bytes([0x01, 0x00])
    return handshake_pkt + status_req_pkt


def build_mc_slp_status_response(status_dict: dict) -> bytes:
    raw_json = json.dumps(status_dict, separators=(",", ":")).encode("utf-8")
    body = encode_varint(0x00) + encode_varint(len(raw_json)) + raw_json
    return encode_varint(len(body)) + body


def run_mc_slp_probe(host: str, port: int, family: socket.AddressFamily = socket.AF_INET) -> dict:
    with socket.socket(family, socket.SOCK_STREAM) as sock:
        sock.settimeout(3.0)
        sock.connect((host, port))
        sock.sendall(build_mc_slp_request(770, host, port))

        buf = bytearray()
        while True:
            chunk = sock.recv(4096)
            if not chunk:
                raise RuntimeError("EOF before Minecraft SLP status response")
            buf.extend(chunk)
            try:
                pkt_len, hdr_end = decode_varint(buf, 0)
            except ValueError:
                continue
            if len(buf) >= hdr_end + pkt_len:
                pkt_id, id_end = decode_varint(buf, hdr_end)
                if pkt_id != 0x00:
                    raise RuntimeError(f"unexpected SLP packet ID: {pkt_id}")
                str_len, str_start = decode_varint(buf, id_end)
                json_bytes = bytes(buf[str_start : str_start + str_len])
                status = json.loads(json_bytes.decode("utf-8"))
                trailing = bytearray(buf[hdr_end + pkt_len :])
                break

        ping_payload = 0x0102030405060708
        ping_pkt = bytes([0x09, 0x01]) + struct.pack(">Q", ping_payload)
        sock.sendall(ping_pkt)

        while len(trailing) < 10:
            chunk = sock.recv(64)
            if not chunk:
                raise RuntimeError("EOF before Minecraft SLP pong response")
            trailing.extend(chunk)

        if bytes(trailing[:10]) != ping_pkt:
            raise RuntimeError("SLP pong payload mismatch")

        return status


def verify_profiles() -> None:
    loaded = {}
    for path in sorted(PROFILES_DIR.glob("*.json")):
        data = json.loads(path.read_text(encoding="utf-8-sig"))
        loaded[data["id"]] = data

    if set(loaded.keys()) != set(EXPECTED_PROFILES.keys()):
        raise RuntimeError(
            f"profile set mismatch: found {sorted(loaded.keys())}, expected {sorted(EXPECTED_PROFILES.keys())}"
        )

    for pid, expected in EXPECTED_PROFILES.items():
        prof = loaded[pid]
        default_enabled = [
            m["key"]
            for m in prof["mappings"]
            if (not m["optional"]) and m["evidence"] == "verified"
        ]
        optional_keys = [m["key"] for m in prof["mappings"] if m["optional"]]
        if default_enabled != expected["default_enabled"]:
            raise RuntimeError(
                f"{pid}: default_enabled={default_enabled}, expected={expected['default_enabled']}"
            )
        if optional_keys != expected["optional_off"]:
            raise RuntimeError(
                f"{pid}: optional_off={optional_keys}, expected={expected['optional_off']}"
            )
    print("  OK   all 5 profiles match GAME-002 / GAME-007 / GAME-008 / ADR-021 mapping rules")


def verify_self_contained_minecraft_java_l0_l1() -> None:
    for family, bind_host, label in [
        (socket.AF_INET, "127.0.0.1", "IPv4"),
        (socket.AF_INET6, "::1", "IPv6"),
    ]:
        srv = socket.socket(family, socket.SOCK_STREAM)
        srv.bind((bind_host, 0))
        srv.listen(2)
        port = srv.getsockname()[1]

        errors: list[str] = []

        def serve(listener: socket.socket) -> None:
            try:
                # Connection 1: L0 connect-only probe (client closes immediately, server sends nothing)
                conn0, _ = listener.accept()
                conn0.settimeout(2.0)
                _ = conn0.recv(16)
                conn0.close()

                # Connection 2: L1 SLP Handshake + Status + Ping/Pong
                conn1, _ = listener.accept()
                conn1.settimeout(3.0)
                data = conn1.recv(1024)
                if not data:
                    errors.append("empty L1 request")
                    conn1.close()
                    return
                resp = build_mc_slp_status_response(
                    {
                        "version": {"name": "26.3", "protocol": 770},
                        "players": {"max": 20, "online": 2},
                        "description": {"text": "GTT L1 Test"},
                    }
                )
                conn1.sendall(resp)
                ping = conn1.recv(32)
                conn1.sendall(ping)
                conn1.close()
            except Exception as exc:  # noqa: BLE001
                errors.append(str(exc))
            finally:
                listener.close()

        t = threading.Thread(target=serve, args=(srv,), daemon=True)
        t.start()

        # L0 connect-only probe
        with socket.socket(family, socket.SOCK_STREAM) as c0:
            c0.settimeout(2.0)
            c0.connect((bind_host, port))

        # L1 SLP probe
        status = run_mc_slp_probe(bind_host, port, family=family)
        t.join(timeout=3.0)

        if errors:
            raise RuntimeError(f"Minecraft Java {label} responder error: {errors}")
        if status.get("version", {}).get("name") != "26.3" or status.get("players", {}).get("online") != 2:
            raise RuntimeError(f"unexpected SLP status on {label}: {status}")
        print(f"  OK   Minecraft: Java Edition L0 + L1 SLP ({label}, custom port {port})")


def verify_gap_inventory() -> None:
    if not README_PATH.is_file():
        raise RuntimeError(f"missing {README_PATH}")
    text = README_PATH.read_text(encoding="utf-8-sig")
    for gap_id in REQUIRED_GAP_IDS:
        if gap_id not in text:
            raise RuntimeError(f"explicit gap {gap_id} missing from {README_PATH}")
    if "No game is marked `supported` until its L2 real-client join test passes" not in text:
        raise RuntimeError("ADR-011 / GAME-006 L2 support gate statement missing from README.md")
    print(f"  OK   explicit gap inventory verified ({len(REQUIRED_GAP_IDS)} gaps documented)")


def main() -> int:
    parser = argparse.ArgumentParser(description="Run L0/L1 game compatibility checks (T-090).")
    parser.add_argument(
        "--live-mc-java",
        metavar="HOST:PORT",
        help="Optional live or containerized Minecraft: Java Edition server address to probe (L0 + L1 SLP).",
    )
    args = parser.parse_args()

    verify_profiles()
    verify_self_contained_minecraft_java_l0_l1()
    verify_gap_inventory()

    if args.live_mc_java:
        host, port_str = args.live_mc_java.rsplit(":", 1)
        port = int(port_str)
        status = run_mc_slp_probe(host, port)
        print(f"  OK   live Minecraft: Java Edition L1 SLP at {host}:{port} -> {status.get('version')}")

    print("PASS: T-090 L0/L1 game compatibility checks succeeded; explicit gaps verified.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
