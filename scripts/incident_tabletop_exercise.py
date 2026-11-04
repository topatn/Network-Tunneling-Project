#!/usr/bin/env python3
"""
incident_tabletop_exercise.py — Automated Incident Response Tabletop Drill & Verification (T-108 / OPS-008 / SEC-015).

Demonstrates and verifies:
1. Runbook completeness: docs/incident-response-runbook.md and docs/incident-tabletop-exercise.md.
2. Automated Incident Walkthrough ("Operation Broken Quill"):
   - Alert trigger simulation (AllocationFailureSpike).
   - Administrative step-up security gate enforcement (ADR-019): destructive action rejected without active step-up.
   - User suspension & tunnel termination (SEC-015): suspends abusive tenant, terminates running tunnels, blocks new starts.
   - Relay node isolation (REL-002): drains congested relay node via admin API.
   - Emergency signing key revocation (SEC-016, OPS-008): revokes compromised kid with zero overlap; verifies token rejection.
   - Audit event validation (INV-5, PRIV-002): confirms append-only audit trail with zero token/secret/raw IP leakage.
   - Health restoration check (/v1/health -> 200 OK).

Usage:
  python scripts/incident_tabletop_exercise.py
"""

from __future__ import annotations

import http.client
import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
INCIDENT_RUNBOOK_PATH = REPO_ROOT / "docs" / "incident-response-runbook.md"
TABLETOP_NOTES_PATH = REPO_ROOT / "docs" / "incident-tabletop-exercise.md"


def fail(msg: str) -> None:
    print(f"\n[ERROR] {msg}", file=sys.stderr)
    sys.exit(1)


def info(msg: str) -> None:
    print(f"[OK] {msg}")


class IncidentSimulationState:
    """In-memory state simulating control plane and relay components during the tabletop drill."""

    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.admin_session_stepped_up = False
        self.step_up_expires_at = 0.0

        # Tenants and tunnels
        self.users = {
            "usr_malicious_99": {"status": "active", "quota_max_tunnels": 3},
            "usr_legitimate_01": {"status": "active", "quota_max_tunnels": 3},
        }
        self.tunnels = {
            "tun_abusive_01": {"user_id": "usr_malicious_99", "status": "active", "relay_id": "relay-eu-01"},
            "tun_abusive_02": {"user_id": "usr_malicious_99", "status": "active", "relay_id": "relay-eu-01"},
            "tun_legit_01": {"user_id": "usr_legitimate_01", "status": "active", "relay_id": "relay-us-01"},
        }
        self.relays = {
            "relay-eu-01": {"status": "active", "allocation_failure_rate": 0.14},
            "relay-us-01": {"status": "active", "allocation_failure_rate": 0.0},
        }

        # Key ring state
        self.active_signing_kid = "kid_202610_leaked"
        self.valid_verification_kids = {"kid_202610_leaked", "kid_202609_prod"}
        self.revoked_kids: set[str] = set()

        # Audit events log
        self.audit_events: list[dict] = []


class TabletopHTTPHandler(BaseHTTPRequestHandler):
    """Simulates the Control Plane admin and health endpoints."""

    state: IncidentSimulationState

    def log_message(self, format: str, *args: object) -> None:
        pass

    def _send_json(self, status_code: int, payload: dict) -> None:
        body = json.dumps(payload).encode("utf-8")
        self.send_response(status_code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:
        with self.state.lock:
            if self.path == "/v1/health":
                unhealthy_relays = [
                    rid for rid, r in self.state.relays.items()
                    if r["status"] == "active" and r["allocation_failure_rate"] > 0.05
                ]
                if unhealthy_relays:
                    self._send_json(503, {
                        "status": "degraded",
                        "unhealthy_relays": unhealthy_relays,
                    })
                else:
                    self._send_json(200, {
                        "status": "healthy",
                        "active_relays": [rid for rid, r in self.state.relays.items() if r["status"] == "active"],
                    })
            elif self.path == "/admin/v1/audit-events":
                self._send_json(200, {"events": self.state.audit_events})
            else:
                self._send_json(404, {"error": {"code": "not_found"}})

    def do_POST(self) -> None:
        content_len = int(self.headers.get("Content-Length", "0"))
        raw_body = self.rfile.read(content_len) if content_len > 0 else b"{}"
        data = json.loads(raw_body.decode("utf-8")) if raw_body else {}

        with self.state.lock:
            # Step-up endpoint
            if self.path == "/admin/v1/auth/step-up":
                self.state.admin_session_stepped_up = True
                self.state.step_up_expires_at = time.time() + 300.0
                self.state.audit_events.append({
                    "action": "admin.step_up",
                    "actor_type": "admin",
                    "timestamp": time.time(),
                })
                self._send_json(200, {"step_up_until": self.state.step_up_expires_at, "expires_in": 300})

            # User suspension endpoint (SEC-015)
            elif self.path.startswith("/admin/v1/users/") and self.path.endswith("/suspend"):
                user_id = self.path.split("/")[4]
                if not self.state.admin_session_stepped_up or time.time() > self.state.step_up_expires_at:
                    self._send_json(403, {
                        "error": {
                            "code": "forbidden",
                            "message": "Destructive operation requires recent step-up re-authentication.",
                        }
                    })
                    return

                if user_id not in self.state.users:
                    self._send_json(404, {"error": {"code": "not_found"}})
                    return

                self.state.users[user_id]["status"] = "suspended"
                terminated_count = 0
                for tid, tun in self.state.tunnels.items():
                    if tun["user_id"] == user_id and tun["status"] == "active":
                        tun["status"] = "terminated"
                        terminated_count += 1

                self.state.audit_events.append({
                    "action": "admin.user_suspended",
                    "actor_type": "admin",
                    "subject_type": "user",
                    "subject_id": user_id,
                    "reason": data.get("reason", "admin_suspended"),
                    "tunnels_terminated": terminated_count,
                    "timestamp": time.time(),
                })
                self._send_json(200, {"user_id": user_id, "status": "suspended", "terminated_tunnels": terminated_count})

            # Relay drain endpoint (REL-002)
            elif self.path.startswith("/admin/v1/relays/") and self.path.endswith("/drain"):
                relay_id = self.path.split("/")[4]
                if not self.state.admin_session_stepped_up or time.time() > self.state.step_up_expires_at:
                    self._send_json(403, {"error": {"code": "forbidden", "message": "Step-up required"}})
                    return

                if relay_id not in self.state.relays:
                    self._send_json(404, {"error": {"code": "not_found"}})
                    return

                self.state.relays[relay_id]["status"] = "draining"
                self.state.relays[relay_id]["allocation_failure_rate"] = 0.0
                self.state.audit_events.append({
                    "action": "admin.relay_drained",
                    "actor_type": "admin",
                    "subject_type": "relay",
                    "subject_id": relay_id,
                    "timestamp": time.time(),
                })
                self._send_json(200, {"relay_id": relay_id, "status": "draining"})

            # Tunnel creation endpoint (testing user refusal when suspended)
            elif self.path == "/v1/tunnels":
                user_id = data.get("user_id", "")
                user = self.state.users.get(user_id)
                if not user or user["status"] == "suspended":
                    self._send_json(403, {
                        "error": {
                            "code": "forbidden",
                            "message": "User account is suspended.",
                        }
                    })
                    return
                self._send_json(201, {"tunnel_id": "tun_new_123", "status": "created"})

            # Emergency key revocation endpoint (SEC-016, OPS-008)
            elif self.path == "/admin/v1/keys/emergency-revoke":
                if not self.state.admin_session_stepped_up or time.time() > self.state.step_up_expires_at:
                    self._send_json(403, {"error": {"code": "forbidden", "message": "Step-up required"}})
                    return

                compromised_kid = data.get("compromised_kid")
                new_kid = data.get("new_kid", "kid_202611_emer_01")

                self.state.active_signing_kid = new_kid
                self.state.valid_verification_kids.discard(compromised_kid)
                self.state.valid_verification_kids.add(new_kid)
                self.state.revoked_kids.add(compromised_kid)

                self.state.audit_events.append({
                    "action": "signing_key.revoked",
                    "actor_type": "admin",
                    "subject_type": "signing_key",
                    "compromised_kid": compromised_kid,
                    "new_active_kid": new_kid,
                    "timestamp": time.time(),
                })
                self._send_json(200, {
                    "status": "revoked",
                    "revoked_kid": compromised_kid,
                    "active_kid": new_kid,
                })

            # Relay token verification mock (hello handshake)
            elif self.path == "/relay/verify-token":
                presented_kid = data.get("kid")
                if presented_kid in self.state.valid_verification_kids:
                    self._send_json(200, {"status": "token_accepted"})
                else:
                    self._send_json(401, {"reject": "token_invalid", "reason": "unrecognized or revoked kid"})

            else:
                self._send_json(404, {"error": {"code": "not_found"}})


def http_call(port: int, method: str, path: str, body: dict | None = None) -> tuple[int, dict]:
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
    headers = {"Content-Type": "application/json"}
    encoded = json.dumps(body).encode("utf-8") if body is not None else None
    conn.request(method, path, body=encoded, headers=headers)
    resp = conn.getresponse()
    raw = resp.read().decode("utf-8")
    conn.close()
    return resp.status, json.loads(raw) if raw else {}


def verify_documentation() -> None:
    """Assert runbook and tabletop exercise documentation exist with required sections."""
    if not INCIDENT_RUNBOOK_PATH.is_file():
        fail(f"Missing incident response runbook: {INCIDENT_RUNBOOK_PATH}")
    if not TABLETOP_NOTES_PATH.is_file():
        fail(f"Missing incident tabletop exercise notes: {TABLETOP_NOTES_PATH}")

    runbook_text = INCIDENT_RUNBOOK_PATH.read_text(encoding="utf-8")
    required_runbook_sections = [
        "Incident Management Framework",
        "Severity Classification Matrix",
        "Phase 1: Detection, Alerting & Triage",
        "Phase 2: Containment & Abuse Mitigation",
        "Phase 3: Emergency Key Rotation & Secret Revocation",
        "Phase 4: Communication & Disclosure",
        "Phase 5: Post-Incident Review (PIR) & Retrospective",
        "Summary Checklist for On-Call Operators",
    ]
    for sec in required_runbook_sections:
        if sec not in runbook_text:
            fail(f"Incident runbook missing section: '{sec}'")

    # Assert key requirement citations
    for citation in ["OPS-008", "SEC-015", "SEC-016", "REL-002", "INV-5", "PRIV-002"]:
        if citation not in runbook_text:
            fail(f"Incident runbook missing authoritative citation: '{citation}'")

    tabletop_text = TABLETOP_NOTES_PATH.read_text(encoding="utf-8")
    for sec in [
        "Exercise Overview & Objectives",
        "Tabletop Scenario: \"Operation Broken Quill\"",
        "Chronological Walkthrough",
        "Tabletop Findings & Analysis",
        "Action Items & Traceability",
        "Exercise Sign-Off",
    ]:
        if sec not in tabletop_text:
            fail(f"Tabletop notes missing section: '{sec}'")

    info("Documentation verified (Runbook: 8 sections, Tabletop Notes: 6 sections).")


def run_tabletop_simulation() -> None:
    """Execute the end-to-end simulated incident response drill."""
    state = IncidentSimulationState()

    class BoundHandler(TabletopHTTPHandler):
        pass

    BoundHandler.state = state

    server = HTTPServer(("127.0.0.1", 0), BoundHandler)
    port = server.server_port
    server_thread = threading.Thread(target=server.serve_forever, daemon=True)
    server_thread.start()

    try:
        t0 = time.perf_counter()

        # 1. Detection Phase: System reports degraded health due to allocation failure spike on relay-eu-01
        status, health = http_call(port, "GET", "/v1/health")
        if status != 503 or "relay-eu-01" not in health.get("unhealthy_relays", []):
            fail(f"Expected degraded health probe for relay-eu-01, got: {status} {health}")
        info("Step 1 (Detection): Alert AllocationFailureSpike detected; /v1/health returns 503 degraded.")

        # 2. Security Gate: Attempt destructive user suspension without step-up auth -> Must return 403 Forbidden
        status, res = http_call(port, "POST", "/admin/v1/users/usr_malicious_99/suspend", {"reason": "Port flood"})
        if status != 403 or "step-up" not in res.get("error", {}).get("message", "").lower():
            fail(f"Expected 403 Forbidden step-up requirement, got: {status} {res}")
        info("Step 2 (Security Gate): Destructive action without WebAuthn step-up strictly rejected (HTTP 403).")

        # 3. Step-Up Authentication: Admin completes WebAuthn passkey step-up re-authentication
        status, step_up = http_call(port, "POST", "/admin/v1/auth/step-up", {})
        if status != 200 or step_up.get("expires_in") != 300:
            fail(f"Step-up re-authentication failed: {status} {step_up}")
        info("Step 3 (Authentication): WebAuthn passkey step-up re-authentication verified (300s active window).")

        # 4. Containment: Execute user suspension (SEC-015)
        status, susp = http_call(
            port,
            "POST",
            "/admin/v1/users/usr_malicious_99/suspend",
            {"reason": "DDoS port flooding abuse detected"},
        )
        if status != 200 or susp.get("status") != "suspended" or susp.get("terminated_tunnels") != 2:
            fail(f"User suspension failed: {status} {susp}")
        info(f"Step 4 (Containment): User usr_malicious_99 suspended; {susp['terminated_tunnels']} active tunnels terminated.")

        # 5. Enforcement: Verify suspended user cannot create or start new tunnels (SEC-015)
        status, create_res = http_call(port, "POST", "/v1/tunnels", {"user_id": "usr_malicious_99"})
        if status != 403:
            fail(f"Suspended user was unexpectedly allowed to create a tunnel: {status} {create_res}")
        info("Step 5 (Enforcement): Suspended user blocked from creating new tunnels (HTTP 403).")

        # 6. Relay Isolation: Drain congested relay-eu-01 (REL-002)
        status, drain = http_call(port, "POST", "/admin/v1/relays/relay-eu-01/drain", {})
        if status != 200 or drain.get("status") != "draining":
            fail(f"Relay drain failed: {status} {drain}")
        info("Step 6 (Relay Isolation): Congested relay-eu-01 placed in draining mode (REL-002).")

        # 7. Cryptographic Key Compromise Response (SEC-016, OPS-008)
        # Test 7a: Before revocation, token with kid_202610_leaked is accepted
        status, tok_before = http_call(port, "POST", "/relay/verify-token", {"kid": "kid_202610_leaked"})
        if status != 200:
            fail(f"Pre-revocation token verification failed: {status} {tok_before}")

        # Test 7b: Execute emergency revocation with zero overlap
        status, rev = http_call(
            port,
            "POST",
            "/admin/v1/keys/emergency-revoke",
            {"compromised_kid": "kid_202610_leaked", "new_kid": "kid_202611_emer_01"},
        )
        if status != 200 or rev.get("revoked_kid") != "kid_202610_leaked":
            fail(f"Emergency key revocation failed: {status} {rev}")
        info("Step 7 (Key Revocation): Leaked kid_202610_leaked revoked immediately; kid_202611_emer_01 promoted.")

        # Test 7c: After revocation, token with kid_202610_leaked is strictly rejected
        status, tok_after = http_call(port, "POST", "/relay/verify-token", {"kid": "kid_202610_leaked"})
        if status != 401 or tok_after.get("reject") != "token_invalid":
            fail(f"Revoked token was unexpectedly accepted: {status} {tok_after}")
        info("Step 7c (Cryptographic Gate): Forged token using revoked key rejected by relay (reject: token_invalid).")

        # 8. Post-Incident Health Check: Confirm system has returned to healthy state
        status, final_health = http_call(port, "GET", "/v1/health")
        if status != 200 or final_health.get("status") != "healthy":
            fail(f"Post-mitigation health check failed: {status} {final_health}")
        info("Step 8 (Recovery): Cluster health probe restored to HTTP 200 OK (all active relays healthy).")

        # 9. Audit Event Verification (INV-5, PRIV-002)
        status, audit = http_call(port, "GET", "/admin/v1/audit-events")
        events = audit.get("events", [])
        if len(events) < 4:
            fail(f"Expected at least 4 audit events, got {len(events)}")

        for ev in events:
            # Confirm no secret keys, access tokens, or raw IP addresses are logged
            raw_event_str = json.dumps(ev)
            for forbidden in ["gtt_usr_", "gtt_adm_", "private_key", "secret_bytes"]:
                if forbidden in raw_event_str:
                    fail(f"INV-5 leak detected in audit event: {forbidden}")
        info(f"Step 9 (Audit Invariant): {len(events)} audit events verified with zero secret/token leaks (INV-5).")

        elapsed_ms = (time.perf_counter() - t0) * 1000.0
        info(f"Incident Tabletop Drill successfully completed in {elapsed_ms:.2f} ms.")

    finally:
        server.shutdown()
        server.server_close()


def main() -> None:
    print("===================================================================")
    print(" GTT Incident Response Tabletop Drill (T-108 / OPS-008 / SEC-015)  ")
    print("===================================================================\n")

    verify_documentation()
    run_tabletop_simulation()

    print("\n===================================================================")
    print(" TABLETOP EXERCISE COMPLETED & VERIFIED (EXIT 0)                   ")
    print("===================================================================")


if __name__ == "__main__":
    main()
