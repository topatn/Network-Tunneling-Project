# Game Server Tunneling Tool (GTT)

**Status:** Implementation & Verification Phase. The core architecture, desktop application shell, in-process engine bridge, control plane, relay node, protocol framing, game profiles, and automated testing suites are built and verified locally and in CI.

> **Support claim discipline (`research.md` ADR-011, `GAME-006`, `AGENTS.md` §6.6):** Passing L0 (connect reachability) and L1 (status protocol) tests proves transport forwarding through the tunnel. **No game is marked `supported` until its manual L2 real-client join test passes (`T-091`).**

---

## 1. What It Is

A Windows desktop application that lets people host multiplayer game servers on their own PC and share them with friends over the internet without router port forwarding, public IP addresses, or NAT configuration.

- **Host Experience:** The host chooses a game profile, customizes ports or settings, starts a tunnel, and shares a private invitation link.
- **Guest Experience:** Guests claim the invitation link via a web browser; the relay admits the guest's IP address on a strict default-deny basis (`SEC-009`) and forwards game traffic over authenticated UDP and TCP flows.
- **In-Process Engine Bridge:** The native desktop shell communicates directly with the tunnel engine lifecycle across loopback (`127.0.0.1`) with zero external service leakage (`contracts.md` §13, `INV-1`).
- **Integrated Server Management:** Supports verified one-click provisioning and child-process supervision for games like Minecraft: Java Edition (`SRV-001`–`SRV-018`).

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

## 3. Installation Guide

GTT is designed to run with **zero administrator privileges** (`SEC-012`). All application binaries, per-user configurations, and game server directories are strictly confined to `%LOCALAPPDATA%` (`SRV-010`, `INST-001`).

### 3.1 System Requirements

- **Operating System:** Windows 10 (version 1809+) or Windows 11 64-bit.
- **Web Browser / Runtime:** Microsoft Edge or Microsoft Edge WebView2 Evergreen Runtime (pre-installed on Windows 10 and 11).
- **Java Runtime (Optional):** Java 25 or newer (`SRV-018`). Only required if using the built-in managed Minecraft: Java Edition server.
- **Network:** Standard broadband internet connection. Router port forwarding and static public IPs are **not** needed.

### 3.2 Method A: Quick Start from Source (Developer Mode)

To build and run the application directly:

```powershell
# 1. Ensure Rust toolchain is available (stable x86_64)
rustc --version
cargo --version

# 2. Run the desktop application with embedded assets and local engine bridge
cargo run -p gtt-desktop
```

GTT will:
1. Start the native engine HTTP bridge on `http://127.0.0.1:5173`.
2. Automatically launch the desktop standalone window using Edge / WebView2.

### 3.3 Method B: Per-User Installation (`INST-001`, `INST-002`)

To install GTT as a standalone desktop app for the current Windows user without elevation:

```powershell
# 1. Build the release binary
cargo build --release -p gtt-desktop

# 2. Execute per-user installation into %LOCALAPPDATA%\Programs\GTT\
.\target\release\gtt-desktop.exe --install-per-user "$env:LOCALAPPDATA"
```

**Where files are stored:**
- **Executable:** `%LOCALAPPDATA%\Programs\GTT\gtt-desktop.exe`
- **Start Menu Shortcut:** `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Game Tunnel Tooling.lnk`
- **Server Worlds & Data:** `%LOCALAPPDATA%\GTT\servers\`
- **Application Logs & State:** `%LOCALAPPDATA%\GTT\`

### 3.4 Verifying the Installation

```powershell
# Verify embedded assets and asInvoker security manifest (exit 0)
gtt-desktop.exe --self-check

# Verify WebView2 browser runtime status
gtt-desktop.exe --verify-webview2
```

### 3.5 Uninstalling GTT (`INST-004`)

```powershell
# Remove application executable and Start Menu shortcut (preserves your worlds and data)
gtt-desktop.exe --uninstall-per-user "$env:LOCALAPPDATA"

# Completely remove all files including worlds, server data, and credentials
gtt-desktop.exe --uninstall-per-user "$env:LOCALAPPDATA" --remove-user-data
```

### 3.6 Linux Distribution Packages (.deb, tarball, AppImage, systemd)

GTT packages natively for Linux desktop and headless servers:

```bash
# Debian / Ubuntu package (.deb)
sudo apt install ./gtt_0.1.0_amd64.deb

# Standalone binary tarball (installs to /usr/local/bin or ~/.local/bin)
tar -xzf gtt-v0.1.0-linux-x86_64.tar.gz && cd gtt-v0.1.0 && ./install.sh

# Headless server daemon (systemd service included)
gtt --headless --port 5173
```

Detailed Linux instructions are in [`docs/linux-distribution-guide.md`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/docs/linux-distribution-guide.md).

---

## 4. User Guide: How to Use GTT

### Step 1: Launch the Application

Launch GTT from the Start Menu or via PowerShell:
```powershell
cargo run -p gtt-desktop
```
The GTT dashboard will open in a clean, dark-themed desktop window.

### Step 2: Choose Your Game Profile

Click **Switch Game** or **Choose Game** on the dashboard. GTT comes with pre-configured profiles:
- **Minecraft: Java Edition** (Default: TCP port 25565)
- **Minecraft: Bedrock Edition** (Default: UDP port 19132)
- **Terraria** (Default: TCP port 7777)
- **Left 4 Dead 2** (Default: UDP port 27015)

Select **Minecraft: Java Edition**.

### Step 3: Set Up Your Game Server

You can host a server in two ways:

#### Option A: Built-in Managed Server ("Set this up for me")
GTT can verify, configure, and supervise your server process automatically:
1. Under **Local Server Configuration**, click **"Set this up for me"**.
2. **Server Jar Path:** GTT defaults to `C:\Users\<YourUsername>\Downloads\server.jar`, or enter your custom path.
3. **Integrity Verification:** GTT streams the jar and verifies its SHA-256 hash (`SRV-002`) against Mojang's published release manifest before executing.
4. **Accept EULA:** Review and accept the official Minecraft End User License Agreement (`SRV-015`).
5. **Start Server:** Click **Start Server**. GTT validates Java 25+, spawns the server process strictly bound to `127.0.0.1:25565`, and tracks its readiness marker (`Done (X.Xs)!`).
6. **Live Console:** Monitor startup progress and logs in real time via the integrated console window.

#### Option B: External / Hand-Managed Server
If you already run a local server (vanilla, Paper, Fabric, Forge, Bedrock, Terraria, etc.):
1. Start your dedicated server normally on your PC.
2. In GTT, leave the local port as `25565` (or enter your custom port).
3. GTT's local probe indicator will confirm that `127.0.0.1:25565` is reachable.

### Step 4: Connecting as the Host

Because the server is running locally on your computer:
1. Open your game client (e.g., Minecraft: Java Edition).
2. Go to **Multiplayer** → **Direct Connection** (or **Add Server**).
3. In the Server Address field, enter:
   ```text
   localhost
   ```
   *(or `127.0.0.1`)*.
4. Click **Join Server**.

> **Managing Whitelist & Operators:**
> Managed server files are located at `%LOCALAPPDATA%\GTT\servers\minecraft-java\`.
> To whitelist players or grant operator permissions, edit `whitelist.json` and `ops.json` in that directory.

### Step 5: Sharing with Friends Over the Internet

GTT supports two ways to share your local server with friends without port-forwarding:

#### Mode 1: Free Tier via Tailscale Node Sharing (`ADR-022`)
Uses Tailscale's encrypted WireGuard mesh network directly between PCs:
1. Ensure Tailscale is signed in on your PC:
   ```powershell
   tailscale up
   ```
2. Open the Tailscale Admin Console ([login.tailscale.com/admin/machines](https://login.tailscale.com/admin/machines)).
3. Locate this PC, click the `...` menu, and select **Share machine...** to generate an invite link.
4. Send the invite link to your friend.
5. Once your friend accepts, they connect in Minecraft using your Tailscale IP:
   ```text
   <your-tailscale-ip>:25565
   ```
   *(e.g. `100.85.120.44:25565` or your Tailscale machine name).*

#### Mode 2: GTT Managed Relay Tier
Uses GTT high-throughput authenticated relays:
1. Click **Start Tunnel** on the GTT dashboard.
2. GTT establishes an authenticated QUIC connection to the relay and reserves a public port.
3. Click **Create Invitation** to generate a private 128-bit claim link.
4. Send the link to your friend. When they open it in their browser, the relay grants firewall admission for their IP address (`SEC-009`).
5. Your friend copies the public address (e.g. `relay.region.gtt.gg:25565`) into their game client to join.

### Step 6: Graceful Stop

- **Stop Server:** Click **Stop Server**. GTT writes `stop\n` to the server's stdin, allowing Minecraft to save all world chunks to disk before exiting cleanly.
- **Stop Tunnel:** Click **Stop Tunnel** to release relay allocations and close network forwarders.

---

## 5. Game Compatibility Status (`T-090`, `T-091`, `TEST-005`)

| Game | Profile | L0 (Connect) | L1 (Protocol Status) | L2 (Real Client) | Status |
|---|---|:---:|:---:|:---:|---|
| **Minecraft: Java Edition** | `profiles/minecraft-java.json` | **PASS** | **PASS** (SLP `0x00`/`0x01` + UT3 Query) | Open (`G-MCJAVA-L2`) | Verified L0/L1; default-off RCON/Query enforced (`GAME-008`) |
| **Minecraft: Bedrock Edition** | `profiles/minecraft-bedrock.json` | **PASS** | **PASS** (RakNet `0x01 -> 0x1c`) | Open (`G-BEDROCK-L2`) | Verified L0/L1; out of first provisioning cut (`SRV-003`) |
| **Terraria** | `profiles/terraria.json` | **PASS** | Gap (`G-TERRARIA-L1`, `E-1`) | Open (`T-091`) | Verified L0; dedicated server lacks unauthenticated query |
| **Left 4 Dead 2** | `profiles/l4d2.json` | **PASS** | **PASS** (Source `A2S_INFO`) | Open (`G-L4D2-SRCDS`) | Verified L0/L1; per-guest loopback identity tested in `T-013` |
| **Left 4 Dead (1)** | `profiles/l4d1.json` | Gap (`ADR-021`) | Gap (`G-L4D1-EVIDENCE`) | Open (`T-091`) | Reported evidence `[R]`; 0 default-enabled mappings (`GAME-006`) |

*Explicit gap inventory details are documented in [`tests/game-compat/README.md`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/tests/game-compat/README.md).*

---

## 6. Development Environment & Prerequisites

The following toolchain versions are verified in local development and CI:

- **Operating System:** Windows 10/11 x64 (target platform) or Linux (CI runner)
- **Rust Toolchain:** `1.80+` (verified with `stable-x86_64-pc-windows-gnu` and `stable-x86_64-pc-windows-msvc`)
- **Node.js & npm:** Node.js `20+`, npm `10+` (for desktop UI tests and development server)
- **Python:** Python `3.11+` (for automated test harnesses and traceability audit)
- **Database (Control Plane):** PostgreSQL `16` or `17`

---

## 7. Verified Commands

Every command listed below has been executed and verified in this repository.

### 7.1 Code Quality & Formatting

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

### 7.2 Unit & Integration Tests

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

### 7.3 Unified Verification Suites

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

# Linux distribution packaging (.deb, tarball, AppImage, systemd unit)
python scripts/verify_linux_packaging.py

# Staging observability, alerting & egress cost dashboards (T-103, OPS-002, OPS-006)
python scripts/verify_observability.py

# Database backup and point-in-time restore simulation (T-106, OPS-007)
python scripts/backup_restore_test.py --dry-run
```

---

## 8. Security & Invariant Rules

- **Zero Elevation (`SEC-012`):** The desktop application enforces `<requestedExecutionLevel level="asInvoker" uiAccess="false" />` and never requests administrator privileges.
- **Local Target Immutability (`SEC-005`, `SEC-006`, `INV-1`):** The control plane rejects all local destination addresses and ports in requests. Relays never connect to destinations chosen by clients. The engine dials strictly from its own configuration.
- **Default-Off Optional Ports (`GAME-008`):** Optional mappings (e.g. RCON, Query) are unconditionally disabled by default and require explicit host opt-in.
- **Zero Secrets & IP Minimization (`PRIV-002`, `INV-5`):** Raw guest IPs and credentials are never stored in the database, committed, or written to logs. Owner IPs are masked to `/24` (IPv4) or `/48` (IPv6).
- **Dependency Discipline (`AGENTS.md` §5):** All dependencies are recorded in [`docs/dependency-register.md`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/docs/dependency-register.md). `Cargo.lock` is strictly pinned.

---

## 9. Documentation Map

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
| [`docs/linux-distribution-guide.md`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/docs/linux-distribution-guide.md) | Linux packages (.deb, tarball, AppImage), systemd unit, and headless server usage |
| [`AGENTS.md`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/AGENTS.md), [`CLAUDE.md`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/CLAUDE.md) | Workflow rules, safety boundaries, and engineering standards for AI agents |
