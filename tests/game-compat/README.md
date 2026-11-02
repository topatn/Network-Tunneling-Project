# Game Compatibility Automation (`tests/game-compat`, `T-090`)

**Requirements:** `TEST-005`, `GAME-001`, `GAME-002`, `GAME-003`, `GAME-004`, `GAME-005`, `GAME-006`, `GAME-008`, `GAME-009`
**Authoritative references:** `research.md` §2 RQ-1, §5 RQ-12, §5 RQ-13.2c, `ADR-011`, `ADR-013`, `ADR-021`

> **Support claim discipline (`ADR-011`, `GAME-006`, `AGENTS.md` §6.6):**
> Passing L0 and L1 proves transport reachability and game status-protocol framing through the tunnel. **No game is marked `supported` until its L2 real-client join test passes (`T-091`).**

---

## 1. Compatibility Levels (`ADR-011`)

| Level | What it proves | Automation status (`T-090`) |
|---|---|---|
| **L0** | Port reachable and bidirectional bytes forwarded through `Guest -> Relay -> Engine -> Local Server` on default and non-default local ports (`GAME-009`), plus local reachability probe (`FR-011`) | **Automated** (`crates/engine/tests/game_compat.rs` and `tests/game-compat/run_game_compat.py`) |
| **L1** | Game-specific status/query protocol answers through the tunnel | **Automated** for **Minecraft: Java Edition** (TCP Server List Ping `0x00`/`0x01` + optional UDP UT3 Query `\xfe\xfd\x09`), **Minecraft: Bedrock Edition** (UDP RakNet Unconnected Ping `0x01 -> 0x1c`), and **Left 4 Dead 2** (UDP Source `A2S_INFO`). Explicit gaps documented below for Terraria and Left 4 Dead (1) |
| **L2** | Real game client joins through the relay address and plays a session | **Manual per release (`T-091`)** until `E-11` establishes whether any headless/scriptable client exists |

---

## 2. Per-Game Test Plan and Current Status (`TEST-005`)

### 2.1 Minecraft: Java Edition (`profiles/minecraft-java.json`, `GAME-002`) — *Primary Focus (`ADR-013`)*

- **Mappings (`RQ-1` [V]):**
  - `game`: TCP `25565` (`optional: false`, `evidence: verified`, enabled by default).
  - `query`: UDP `25565` (`optional: true`, `evidence: verified`, **disabled by default** per `GAME-008` / `SRV-004`).
  - `rcon`: TCP `25575` (`optional: true`, `evidence: verified`, **disabled by default** per `GAME-002` / `GAME-008` / `SRV-004` / `ADR-015`).
- **L0 Test Plan & Automation:**
  - Connect-only TCP probe (`probe_tcp_connect`) to `127.0.0.1` and `[::1]` on default (`25565`) and custom (`GAME-009`) ports. Per `research.md` §5 RQ-13.2c, the server sends zero bytes on initial TCP connect, so L0 never waits for server-initiated data.
  - Bidirectional TCP stream forwarding through `RelayTcpSession` and `EngineTcpSession`.
- **L1 Test Plan & Automation:**
  - **TCP Server List Ping (SLP):** Client sends VarInt-framed `0x00` Handshake (`protocol_version = 770`, host, port, `next_state = 1`) + `0x00` Status Request (`[0x01, 0x00]`) through the relay's public TCP port; parses the `0x00` JSON Status Response (`version.name`, `version.protocol`, `players.max`, `players.online`); sends `0x01` Ping (8-byte payload) and verifies the echoed `0x01` Pong matches byte-for-byte (`probe_minecraft_java_slp`).
  - **Security & Default-Off Check (`GAME-002`):** Verifies that on a default-configured `minecraft-java` tunnel, dialing the `rcon` mapping index (`2`) returns `TcpOpenStatus::Refused` (`0x01`) with zero connections reaching the local RCON listener, and UDP datagrams to `query` (`1`) are dropped and counted.
  - **Optional UDP Query (`query`):** When explicitly enabled by the host, verifies the UT3 Query handshake (`\xfe\xfd\x09` + 4-byte session ID -> `\x09` challenge token) across `RelayUdpSession <-> EngineUdpSession`.
- **Containerized / Live Server Execution:**
  - Headless server container: `itzg/minecraft-server` (`EULA=TRUE`, `ONLINE_MODE=TRUE`) or a local `server.jar` provisioned via `scripts/verify_clean_vm.py`.
  - Run against a live local/containerized server on `127.0.0.1:25565`:
    ```powershell
    python tests/game-compat/run_game_compat.py --live-mc-java 127.0.0.1:25565
    ```

### 2.2 Terraria (`profiles/terraria.json`, `GAME-001`)

- **Mappings (`RQ-1`):**
  - `game`: TCP `7777` (`optional: false`, `evidence: verified`, enabled by default).
  - `game-udp`: UDP `7777` (`optional: true`, `evidence: reported` `[R]`, disabled by default).
- **L0 Status:** **Automated Pass** — connect-only TCP probe (`ProbeLevel::L0`, `ProbeStatus::Reachable`) and TCP byte forwarding on default and custom ports (`GAME-009`).
- **L1 Status:** **Explicit Gap (`G-TERRARIA-L1`, `E-1`)** — Terraria's dedicated server has no unauthenticated status-ping query protocol comparable to Minecraft SLP or Source `A2S_INFO` (community container health checks use plain TCP connect, `research.md` §5 RQ-12), and whether UDP `7777` is ever used during play remains an open contradiction in primary docs (`E-1`). Therefore, `game-udp` reports `ProbeStatus::Unknown` (`FR-011`).

### 2.3 Minecraft: Bedrock Edition (`profiles/minecraft-bedrock.json`, `GAME-003`)

- **Mappings (`RQ-1` [V]):**
  - `game`: UDP `19132` (`optional: false`, `evidence: verified`, enabled by default).
  - `game-ipv6`: UDP `19133` (`optional: true`, `evidence: verified`, disabled by default).
- **L0 / L1 Status:** **Automated Pass (Protocol Harness)** — RakNet Unconnected Ping (`0x01` + 8B timestamp + 16B offline magic + 8B client GUID) -> Unconnected Pong (`0x1c` + server GUID + 16B magic + `MCPE;...` MOTD) verified through `probe_udp_mapping` (`ProbeLevel::L1`, `ProbeStatus::Reachable`).
- **Explicit Gaps (`G-BEDROCK-CONTAINER`, `E-2`, `E-16`):**
  - Bedrock is out of the first provisioning cut (`ADR-013`, `SRV-003`) because no published checksum/signature mechanism was established (`RQ-13.3`).
  - Console and mobile clients cannot connect by IP:port (`GAME-003` [V]).
  - `transport=nethernet` vs `raknet` non-default port binding (`E-2` / `E-16`) requires a real BDS binary and real Bedrock client (`T-012`, `T-091`).

### 2.4 Left 4 Dead 2 (`profiles/l4d2.json`, `GAME-004`)

- **Mappings (`RQ-1`):**
  - `game`: UDP `27015` (`optional: false`, `evidence: verified`, enabled by default).
  - `rcon`: TCP `27015` (`optional: true`, `evidence: verified`, disabled by default).
  - `sourcetv`: UDP `27020` (`optional: true`, `evidence: verified`, disabled by default).
  - `client`: UDP `27005` (`optional: true`, `evidence: verified`, disabled by default).
- **L0 / L1 Status:** **Automated Pass (Protocol Harness)** — Source Engine `A2S_INFO` (`\xff\xff\xff\xffTSource Engine Query\x00` -> `\xff\xff\xff\xffI...` or `\xff\xff\xff\xffA...` challenge) verified via `UdpProbeKind::SourceA2sInfo`; `rcon` TCP `27015` confirmed off by default (`GAME-008`).
- **Explicit Gaps (`G-L4D2-SRCDS`, `E-3`, `E-4`):**
  - Anonymous SteamCMD download (`app 222860`) is multi-gigabyte and not pulled in standard CI jobs; `srcds_run` per-guest loopback IP behavior (`127.0.0.2` distinct player identity and RCON ban scoping, `research.md` §2.2.1) requires `T-013` (`E-3`) and `T-091` (`L2`).

### 2.5 Left 4 Dead (`profiles/l4d1.json`, `GAME-005`)

- **Mappings (`RQ-1` `[R]`):**
  - `game`: UDP `27015` (`optional: false`, `evidence: reported`).
- **L0 / L1 Status:** **Explicit Gap (`G-L4D1-EVIDENCE`, `E-5`, `ADR-021`)** — Because `l4d1`'s sole mapping currently carries `evidence: "reported"` (`[R]`), `enabled_by_default` is `false` (`ADR-021`, `GAME-006`). Primary-source or packet-capture confirmation (`T-013` / `E-5`) is required before enabling `l4d1` mappings by default.

---

## 3. Explicit Gap Summary Table (`T-090` Done-When)

| Gap ID | Game | Level | Reason / Blocker | Tracking Task / Experiment |
|---|---|---|---|---|
| `G-MCJAVA-L2` | Minecraft: Java Edition | L2 | Real-client join over relay required before marking supported (`ADR-011`, `GAME-006`) | `T-011` (`E-11`), `T-091` |
| `G-TERRARIA-L1` | Terraria | L1 | No unauthenticated status query protocol; UDP 7777 requirement unresolved in primary docs | `T-011` (`E-1`), `T-091` |
| `G-BEDROCK-L2` | Minecraft: Bedrock Edition | L1 (live BDS) / L2 | No published artifact checksum (`SRV-003` / `ADR-013`); `nethernet` vs `raknet` port behavior needs real client | `T-012` (`E-2`), `T-024` (`E-16`), `T-091` |
| `G-L4D2-SRCDS` | Left 4 Dead 2 | L1 (live `srcds`) / L2 | Per-guest loopback source identity (`127.0.0.2`) and lobby join (`GAME-010`) need real `srcds` + client | `T-013` (`E-3`), `T-014` (`E-4`), `T-091` |
| `G-L4D1-EVIDENCE` | Left 4 Dead (1) | L0 / L1 / L2 | Port/protocol evidence is `[R]` (`ADR-021`); exposes 0 default-enabled mappings until capture confirms | `T-013` (`E-5`), `T-091` |

---

## 4. Running the Suite

```powershell
# 1. Run the Rust L0/L1 tunnel compatibility integration tests:
cargo +stable-x86_64-pc-windows-gnu test -p gtt-engine --test game_compat

# 2. Run the standalone Python L0/L1 compatibility & gap-inventory verifier:
python tests/game-compat/run_game_compat.py
```
