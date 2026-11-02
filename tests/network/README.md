# Network Impairment Harness and IPv6 Test Suite (`tests/network`, `T-092`)

**Requirements:** `TEST-003`, `NET-005`, `NET-010`, `FR-008`, `FR-014`, `SEC-009`  
**Authoritative references:** `research.md` §2 RQ-1, §7 ADR-007, §8 E-13, `contracts.md` §§7, 10, 12, `architecture.md` §4

---

## 1. Scope and Objective

Real-world networks are imperfect. Players and hosts operate across cellular connections, consumer Wi-Fi, CGNAT (Carrier-Grade NAT), dual-stack IPv4/IPv6 networks, and restrictive firewalls.

This test harness implements and verifies the behavior of the Game Server Tunneling tool under synthetic and measured network impairments, fulfilling:
- **`TEST-003`:** Network-impairment tests run loss, jitter, latency, and blocked-UDP scenarios, asserting documented behavior.
- **`NET-005`:** Liveness is detected with heartbeats and idle timeouts; silent peer drops transition the session to `Degraded` or `Reconnecting`/`DisconnectedInGrace`.
- **`NET-010`:** IPv4 is fully supported; IPv6 behavior is documented and tested for the claim/game address mismatch case (`E-13`).

---

## 2. Test Scenarios and Documented Behavior

| Scenario ID | Impairment / Condition | Injected Conditions | Expected & Observed Behavior | Requirement |
|---|---|---|---|---|
| **NET-SCEN-01** | **Latency & Jitter Injection** | Base latency 35 ms, spiking to 340 ms (jitter > 250 ms threshold), recovering to 42 ms | • RTT $\le 250\text{ ms}$: Engine remains `Active`.<br>• RTT $> 250\text{ ms}$: Engine detects high latency and transitions to `Degraded` (`NET-005`).<br>• RTT recovers $\le 250\text{ ms}$: Engine returns to `Active`.<br>• Sampled RTT recorded accurately in `last_rtt_ms`. | `TEST-003`, `NET-005` |
| **NET-SCEN-02** | **Packet Loss & Silent-Peer Idle Timeout** | 100% loss of heartbeat `pong` control messages; peer silent | • Silence $\ge 2 \times \text{heartbeat\_interval}$ (10s): Engine transitions `Active -> Degraded`.<br>• Silence $\ge \text{idle\_timeout\_ms}$ (15s): Engine transitions to `Reconnecting`; Relay transitions to `DisconnectedInGrace`.<br>• **Transport split:** TCP streams close immediately (`active_tcp_flows = 0`) per `FR-014`, while UDP flow tables and socket bindings are retained for reconnection (`NET-004`). | `TEST-003`, `NET-005`, `FR-014` |
| **NET-SCEN-03** | **Blocked UDP Transport** | Inbound/outbound UDP packets dropped/filtered (100% loss); TCP remains open | • TCP control stream and game data streams (e.g. Minecraft Java port 25565) remain functional and `Active`.<br>• UDP flows experience packet loss without panics or memory leaks.<br>• Unacknowledged UDP flows expire after `DEFAULT_UDP_FLOW_TIMEOUT_MS` (60s) and are swept cleanly (`active_udp_flows = 0`). | `TEST-003`, `NET-007` |
| **NET-SCEN-04A** | **Outage Resumption within Grace Period** | Simulated transport drop; reconnect attempt within 30s grace window | • Relay transitions to `DisconnectedInGrace { dropped_at_ms, grace_deadline_ms }`.<br>• Reconnect at $t = 22\text{s} < 40\text{s}$ grace deadline succeeds (`resume()`).<br>• Allocation and public ports (`25565`, `19132`) remain identical.<br>• Retained UDP flow resumes on the same local socket without client IP/port churn (`FR-014`). | `NET-004`, `NET-005` |
| **NET-SCEN-04B** | **Grace Period Expiration & Cleanup** | Simulated transport drop; reconnect attempt after grace window | • Time advances past grace deadline ($t = 65\text{s} > 60\text{s}$).<br>• Relay sweeps allocation and transitions to `Released`.<br>• Late reconnect attempt fails; listeners and memory reservations are freed (`NET-008`). | `NET-004`, `NET-008` |
| **NET-SCEN-05A** | **Dual-Stack IPv4 / IPv6 Happy Paths** | Guest claims via IPv4 or IPv6 and connects via matching address family | • IPv4 (`198.51.100.77`) claim + connection: Admitted (`is_guest_admitted == true`).<br>• IPv6 (`2001:db8::cafe:babe`) claim + connection: Admitted.<br>• Tenant isolation enforced: Admission on Tunnel A does not admit on Tunnel B (`SEC-009`). | `NET-010`, `FR-008`, `SEC-009` |
| **NET-SCEN-05B** | **IPv4-Mapped IPv6 Normalization** | Dual-stack socket receives `::ffff:198.51.100.77` | • `canonicalize_guest_ip` normalizes `::ffff:198.51.100.77` to `198.51.100.77`.<br>• Dual-stack guest connection admitted without table fragmentation. | `NET-010`, `data-model.md` §4 |
| **NET-SCEN-05C** | **Address Mismatch: Claim IPv6, Game IPv4** | Guest claims invitation in browser over IPv6 (`2001:db8::dead:beef`), game connects over IPv4 (`203.0.113.99`) | • **Observed & Documented Outcome:** Relay checks admission for `(tunnel_id, 203.0.113.99)`. Because the admission is keyed by IP, the unadmitted IPv4 connection is **refused / dropped**.<br>• See Section 3 for detailed mechanics and remediation. | `NET-010`, `E-13` |
| **NET-SCEN-05D** | **Address Mismatch: Claim IPv4, Game IPv6** | Guest claims invitation in browser over IPv4 (`203.0.113.100`), game connects over IPv6 (`2001:db8::1111`) | • **Observed & Documented Outcome:** Relay checks admission for `(tunnel_id, 2001:db8::1111)`. The unadmitted IPv6 connection is **refused / dropped**. | `NET-010`, `E-13` |
| **NET-SCEN-05E** | **Mobile / CGNAT Address Hopping** | Guest moves from Wi-Fi (`198.51.100.10`) to Cellular (`203.0.113.88`) | • Wi-Fi connection admitted.<br>• Subsequent packets from cellular IP rejected until guest re-claims link from cellular browser.<br>• Re-claiming immediately upserts new IP; play resumes. | `NET-010`, `E-13` |

---

## 3. Deep Dive: IPv4 / IPv6 Claim vs Game Address Mismatch (`NET-010`, `E-13`)

### 3.1 Why Mismatches Occur
In guest model **G2** (`ADR-007`), guests do not install a proprietary client or sign in with an account. Instead, they receive an invitation link (`https://relay.domain/join/{secret}`) and claim it via a standard web browser (`POST /v1/invitations/claim`).

Modern client operating systems use RFC 6724 (Happy Eyeballs v2 / Default Address Selection):
1. **Web Browser (HTTP/HTTPS):** Typically prefers IPv6 when AAAA DNS records exist and IPv6 connectivity is functional. The claim request arrives at the control plane with a source IP of `2001:db8::dead:beef`.
2. **Game Client (Socket):** Many legacy or desktop game engines (e.g. Minecraft Java, Source Engine, Terraria) default to IPv4 sockets (`AF_INET`), or connect to the IPv4 A-record if the game profile specifies an IPv4 relay address. The game connection arrives at the relay with a source IP of `203.0.113.99`.

### 3.2 Observed & Documented Outcome (`NET-010`)
- **Enforcement Rule:** The relay admission enforcer operates on a strict default-deny policy keyed by `(tunnel_id, canonical_guest_ip)`.
- **Outcome:** Because the relay has only received an admission for `2001:db8::dead:beef`, the game connection from `203.0.113.99` is **rejected** (TCP connection closed immediately; UDP datagrams dropped).
- **Security Justification:** Permitting an unadmitted IPv4 connection based on a prior IPv6 claim from an unrelated network would enable unauthorized access and undermine tenant isolation (`SEC-009`, `SEC-005`).

### 3.3 Remediation & Mitigations
1. **Dual-Stack Normalization:** When the relay socket is dual-stack, IPv4-mapped IPv6 addresses (`::ffff:a.b.c.d`) are automatically normalized to native IPv4 (`a.b.c.d`) by `canonicalize_guest_ip`, preventing false mismatches caused by socket implementation details.
2. **Re-Claiming:** Guests encountering a connection failure can re-claim the invitation URL from their game-active network interface. The claim route (`contracts.md` §6) updates/refreshes the admission table idempotently within 1 second.
3. **DNS Family Alignment:** For production relays, configuring the relay hostname to resolve exclusively to IPv4 (or providing distinct IPv4 / IPv6 join endpoints) prevents browser and game client address family divergence.

---

## 4. Running the Tests

### 4.1 Running the Rust Integration Suite
The comprehensive unit and integration suite exercises all 5 impairment scenarios directly against `EngineTunnelSession`, `RelayTunnelSession`, and `RelayAdmissionEnforcer`:

```powershell
cargo test -p gtt-engine --test network_impairment
```

### 4.2 Running the Standalone Network Harness
The standalone Python harness emulates socket-level impairments (latency, jitter, packet drop, blocked UDP, and address mismatches) and verifies test assertions:

```powershell
python tests/network/run_network_tests.py
```
