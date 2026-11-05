# Game Server Tunneling Tool (GTT)

**Status:** Implementation & Verification Phase. The core architecture, desktop application shell, in-process engine bridge, control plane, relay node, protocol framing, game profiles, and automated testing suites are built and verified locally and in CI.

> **Support claim discipline (`research.md` ADR-011, `GAME-006`, `AGENTS.md` §6.6):** Passing L0 (connect reachability) and L1 (status protocol) tests proves transport forwarding through the tunnel. **No game is marked `supported` until its manual L2 real-client join test passes (`T-091`).**

---

## 1. What It Is

A Windows desktop application that lets people host multiplayer game servers on their own PC and share them with friends over the internet without router port forwarding, public IP addresses, or NAT configuration.

- **Host Experience:** The host chooses a game profile, customizes ports or settings, starts a tunnel, and shares a private invitation link.
- **Guest Experience:** Guests claim the invitation link via a web browser; the relay admits the guest's IP address on a strict default-deny basis (`SEC-009`) and forwards game traffic over authenticated UDP and TCP flows.
- **In-Process Engine Bridge:** The native desktop shell communicates directly with the tunnel engine lifecycle across loopback (`127.0.0.1`) with zero external service leakage (`contracts.md` §13, `INV-1`).

---

## 2. Project Architecture

The codebase is organized into modular Rust crates, desktop application frontend assets, database migrations, and automated verification suites:

- [`apps/desktop/`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/apps/desktop/): Windows desktop application shell (WebView2, embedded static assets, native HTTP loopback dispatcher, encrypted credential vault, and `EngineBridge` coordinator).
- [`crates/engine/`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/crates/engine/): Tunnel client engine managing QUIC connections, local target policy dialing, reachability probes, and players-seen counting.
- [`crates/relay/`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/crates/relay/): High-throughput relay node handling token verification, default-deny guest admission enforcement, capacity limits, and TCP/UDP stream forwarding.
- [`crates/control-plane/`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/crates/control-plane/): API server managing accounts, PKCE S256 authentication with token rotation, tunnel allocations, invitations, and audit logging.
- [`crates/protocol/`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/crates/protocol/): Shared wire messages, control framing, session state machines, and RFC 7636 PKCE validation.
- [`crates/profiles/`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/crates/profiles/): Declarative game profile loader and registry.
- [`crates/server-manager/`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/crates/server-manager/): Headless game server provisioning, SHA-256 verification streaming, and filesystem isolation sandbox.
- [`profiles/`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/profiles/): Declarative profiles for Minecraft Java, Bedrock, Terraria, L4D2, and L4D1.
- [`migrations/`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/migrations/): Forward-only PostgreSQL schema migrations for all 14 domain tables.
- [`tests/`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/tests/): Specialized test harnesses for game compatibility, network impairment, security, performance, and privacy.
- [`scripts/`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/scripts/): Automated verification runners and operational drill harnesses.

---

## 3. Game Compatibility Status (`T-090`, `T-091`, `TEST-005`)

| Game | Profile | L0 (Connect) | L1 (Protocol Status) | L2 (Real Client) | Status |
|---|---|:---:|:---:|:---:|---|
| **Minecraft: Java Edition** | `profiles/minecraft-java.json` | **PASS** | **PASS** (SLP `0x00`/`0x01` + UT3 Query) | Open (`G-MCJAVA-L2`) | Verified L0/L1; default-off RCON/Query enforced (`GAME-008`) |
| **Minecraft: Bedrock Edition** | `profiles/minecraft-bedrock.json` | **PASS** | **PASS** (RakNet `0x01 -> 0x1c`) | Open (`G-BEDROCK-L2`) | Verified L0/L1; out of first provisioning cut (`SRV-003`) |
| **Terraria** | `profiles/terraria.json` | **PASS** | Gap (`G-TERRARIA-L1`, `E-1`) | Open (`T-091`) | Verified L0; dedicated server lacks unauthenticated query |
| **Left 4 Dead 2** | `profiles/l4d2.json` | **PASS** | **PASS** (Source `A2S_INFO`) | Open (`G-L4D2-SRCDS`) | Verified L0/L1; per-guest loopback identity tested in `T-013` |
| **Left 4 Dead (1)** | `profiles/l4d1.json` | Gap (`ADR-021`) | Gap (`G-L4D1-EVIDENCE`) | Open (`T-091`) | Reported evidence `[R]`; 0 default-enabled mappings (`GAME-006`) |

*Explicit gap inventory details are documented in [`tests/game-compat/README.md`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/tests/game-compat/README.md).*

---

## 4. Development Environment & Prerequisites

The following toolchain versions are verified in local development and CI:

- **Operating System:** Windows 10/11 x64 (target platform) or Linux (CI runner)
- **Rust Toolchain:** `1.80+` (verified with `stable-x86_64-pc-windows-gnu` and `stable-x86_64-pc-windows-msvc`)
- **Node.js & npm:** Node.js `20+`, npm `10+` (for desktop UI tests and development server)
- **Python:** Python `3.11+` (for automated test harnesses and traceability audit)
- **Database (Control Plane):** PostgreSQL `16` or `17`

---

## 5. Verified Commands

Every command listed below has been executed and verified in this repository.

### 5.1 Code Quality & Formatting

```powershell
# Check code formatting (exit 0, 0 diff)
cargo fmt --all -- --check

# Format codebase
cargo fmt --all

# Workspace-wide Clippy with deny warnings (exit 0, 0 warnings)
cargo clippy --workspace --all-targets -- -D warnings

# Requirements and architecture traceability audit (exit 0, 0 errors)
python scripts/audit_traceability.py

# Traceability audit self-test detecting planted defects
python scripts/audit_traceability.py --self-test

# WCAG AA strict colour contrast verification (UX-005)
python scripts/check_contrast.py --strict
```

### 5.2 Unit & Integration Tests

```powershell
# Run the entire workspace unit and integration test suite
cargo test --workspace --all-targets

# Run desktop UI frontend test harness (8 suites)
npm test --prefix apps/desktop

# Run desktop native packaging and updater lifecycle tests
cargo test -p gtt-desktop --test packaging_lifecycle

# Run desktop native engine bridge integration tests
cargo test -p gtt-desktop --test engine_bridge_integration
```

### 5.3 Unified Verification Suites

```powershell
# Client-side tunnel session & engine bridge verification (T-070, T-073)
python scripts/verify_engine_bridge.py

# PKCE S256 authentication & encrypted credential vault verification (T-040, T-074, T-081)
python scripts/verify_auth_and_credentials.py

# Tunnel lifecycle, relay allocation & quota enforcement (T-042, T-043, T-045)
python scripts/verify_tunnel_lifecycle.py

# Game compatibility L0/L1 automation & gap inventory (T-090, T-035, T-076)
python scripts/verify_game_compat.py

# Network impairment, latency, jitter & IPv4/IPv6 dual-stack (T-092)
python scripts/verify_network_and_resilience.py

# Performance benchmarks & relay capacity limits (T-095, T-096, ADR-023)
python scripts/verify_perf_and_load.py

# Security suite (8 categories), privacy & protocol fuzzing (T-093, T-094, T-098)
python scripts/verify_security_and_privacy.py

# Packaging, signed updater, rollback & uninstall staging (T-100, T-101, T-102)
python scripts/verify_packaging_and_updater.py

# Staging observability, alerting & egress cost dashboards (T-103, OPS-002, OPS-006)
python scripts/verify_observability.py

# Database backup and point-in-time restore simulation (T-106, OPS-007)
python scripts/backup_restore_test.py --dry-run
```

### 5.4 Running the Desktop Application Locally

```powershell
# Run the standalone desktop UI development server (Node.js)
npm run dev --prefix apps/desktop

# Run the native desktop executable with embedded assets and loopback engine bridge
cargo run -p gtt-desktop

# Verify executable self-check and zero-elevation asInvoker policy
cargo run -p gtt-desktop -- --self-check

# Verify WebView2 evergreen runtime detection
cargo run -p gtt-desktop -- --verify-webview2
```

---

## 6. Security & Invariant Rules

- **Zero Elevation (`SEC-012`):** The desktop application enforces `<requestedExecutionLevel level="asInvoker" uiAccess="false" />` and never requests administrator privileges.
- **Local Target Immutability (`SEC-005`, `SEC-006`, `INV-1`):** The control plane rejects all local destination addresses and ports in requests. Relays never connect to destinations chosen by clients. The engine dials strictly from its own configuration.
- **Default-Off Optional Ports (`GAME-008`):** Optional mappings (e.g. RCON, Query) are unconditionally disabled by default and require explicit host opt-in.
- **Zero Secrets & IP Minimization (`PRIV-002`, `INV-5`):** Raw guest IPs and credentials are never stored in the database, committed, or written to logs. Owner IPs are masked to `/24` (IPv4) or `/48` (IPv6).
- **Dependency Discipline (`AGENTS.md` §5):** All dependencies are recorded in [`docs/dependency-register.md`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/docs/dependency-register.md). `Cargo.lock` is strictly pinned.

---

## 7. Documentation Map

| Document | Description |
|---|---|
| [`context.md`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/context.md), [`idea.md`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/idea.md) | Product scope, boundaries, non-goals |
| [`research.md`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/research.md) | Evidence, experiment notes (E-1..E-19), ADR records |
| [`architecture.md`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/architecture.md) | System components, data flows, and trust boundaries |
| [`contracts.md`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/contracts.md), [`api.md`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/api.md) | HTTP and wire protocol contracts, error codes, DTOs |
| [`data-model.md`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/data-model.md), [`schema.md`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/schema.md) | Logical domain model, PostgreSQL physical DDL, and migrations |
| [`requirements.md`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/requirements.md) | Complete inventory of functional, security, and quality requirements |
| [`DESIGN.md`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/DESIGN.md), [`user-flows.md`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/user-flows.md) | UI design system, color tokens, contrast ratios, user journeys |
| [`tasks.md`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/tasks.md), [`plan.md`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/plan.md) | Implementation tasks, prerequisites, and milestone verification checklist |
| [`AGENTS.md`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/AGENTS.md), [`CLAUDE.md`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/CLAUDE.md) | Workflow rules, safety boundaries, and engineering standards for AI agents |
