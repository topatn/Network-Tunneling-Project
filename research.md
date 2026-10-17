# research.md — Research Log and Architecture Decisions

**Status:** DRAFT, partial (Stage 1 + scope change of 2026-10-17). Authoritative source for research evidence and ADRs.
**Last updated:** 2026-10-17

## 0. How to read this document

Every finding carries one of four labels:

- **[V] Verified** — stated by a primary or authoritative source I read.
- **[R] Reported** — stated by third-party guides, forums, or mirrors only. Not yet confirmed against a primary source.
- **[A] Assumption / reasoned recommendation** — architectural judgment, not a sourced fact.
- **[X] Experiment needed** — must be tested before relying on it.

Nothing here is benchmark data. No performance, compatibility, or cost figure has been measured.

**Mechanical verification, 2026-10-17.** All 11 Mermaid diagrams across this document set render with `@mermaid-js/mermaid-cli` **12.0.0**, including the container diagram amended for the server-manager component; a malformed diagram was rejected with a parse error and produced no output, so the renderer is confirmed to reject bad input. Syntax and rendering only — **visual layout has not been reviewed**, and that remains T-004 work. The traceability audit is at `scripts/audit_traceability.py`. The previous report recorded Mermaid 10.9.8 as a parser-only check; rendering was not attempted until now.

## 1. Coverage status

| Required topic (prompt §6 F1) | Status |
|---|---|
| Game networking requirements | Partial (this file, §2) |
| TCP vs UDP forwarding | Partial (§3) |
| Tunnel transport and multiplexing | Partial (§3, §4, §5 RQ-6) |
| NAT traversal approaches | Partial (§4) |
| Relay infrastructure options | Partial (hosting cost drivers only, §5 RQ-8) |
| Desktop framework alternatives | Partial (§5 RQ-5, low-quality sources) |
| Backend framework and database | Partial (§5 RQ-10, reasoning only, no evidence gathered) |
| Authentication and credential storage | Partial (§5 RQ-9; identity provider not researched) |
| Logging and observability | Partial (§5 RQ-11) |
| Packaging and auto-update | Partial (§5 RQ-7) |
| Testing and game-server integration | Partial (§5 RQ-12) |
| Infrastructure and bandwidth cost drivers | Partial (§5 RQ-8) |
| Managed game server acquisition and supervision | Partial (§5 RQ-13.2, §5 RQ-13.3 — desk research only). **Licensing now answered** (§5 RQ-13.1b: EULA and Usage Guidelines read 2026-10-17; hosting permitted, six binding constraints found) |

## 2. Game networking research

### RQ-1: What ports and protocols does each initial game need?

| Game | Port / protocol (defaults) | Label | Notes |
|---|---|---|---|
| Terraria | TCP 7777 default | **[V]** for TCP 7777 (Terraria Wiki "Server", read 2026-10-17: "The server runs on TCP port 7777 by default"). **UDP remains genuinely unresolved and the source contradicts itself** — the same page says that on AWS/GCP "ensure port 7777 on **TCP and UDP** is open". Both claims are on one page. **[X] E-1 stands.** | See §2.1 for provisioning-relevant Terraria findings, including uPNP |
| Minecraft Java | `server-port` **TCP 25565**; `query.port` **UDP 25565** (only with `enable-query=true`); `rcon.port` **TCP 25575** (only with `enable-rcon=true`) | **[V]** for all three ports and for both defaults (**Minecraft Wiki `server.properties`, default file for 26.3, read 2026-10-17**). Mojang publishes no equivalent server reference; the wiki is the best available source and `research.md` §9 already names it as an upgrade target | `enable-rcon=false` and `enable-query=false` by default — **confirms GAME-008**. Note `server-port` and `query.port` **share the number 25565 on different protocols** (TCP vs UDP), so they are not interchangeable. The wiki's own `enable-rcon` text warns RCON is unencrypted and "should only [be connected] from localhost" — independent support for ADR-015 |
| Minecraft Bedrock | UDP; `server-port` 19132 (IPv4), `server-portv6` 19133 (IPv6) | **[V]** port defaults and property set — **two independent sources**: Microsoft's official BDS properties reference, and the full default `server.properties` shipped in BDS 1.26.52.3 (Minecraft Wiki, both read 2026-10-17). [R] for RakNet naming | **Correction to my own 2026-10-17 note:** I earlier wrote that a cited **`server-udp-ports` setting does not appear in the official reference** and called it unverified. **That was wrong.** `server-udp-ports` is real — it is present in the shipped default file (commented out, with documented value syntax and examples). The Microsoft Learn page simply omits it. The original `[R]` claim was sound; I should have checked the shipped file before "correcting" it |
| Left 4 Dead 2 | 27015, UDP for game and queries, TCP for RCON | [V] for Source engine ports, [R] for L4D2 specifics | Valve Developer Community's Source Dedicated Server page lists 27015 TCP/UDP (game, pings, RCON), 27020 UDP (SourceTV), 27005 UDP (client port), 26900 UDP (Steam, outgoing). The wiki is generic to Source, not L4D2-specific. |
| Left 4 Dead (1) | 27015, TCP and UDP per a community Docker image | [R] | Weakest evidence of the five. Needs a primary source or experiment. [X] |

Valve's RCON page states RCON listens on TCP 27015 by default and follows `-port` if changed [V]. Consequence: because TCP and UDP share 27015 on Source servers, a Source profile must let the host choose whether to map TCP at all, and default RCON to **off**.

### RQ-2: Does a Steam lobby / matchmaking join work through a tunnel?

- Reported community guidance: `sv_allow_lobby_connect_only` controls whether direct join (Steam invite, browser, `connect` command) is allowed, and `mm_dedicated_force_servers` is used from a lobby to steer it to a specific dedicated server [R].
- Reported failure mode: "STEAM Validation Rejected" and "session no longer available" errors when joining via lobby [R].
- **Unknown:** whether a lobby join works when the server's address as seen by clients is a relay address instead of the host's real IP. [X]
- **Working hypothesis [A]:** supported scope for L4D/L4D2 v1 is *direct connect by address* (`connect ip:port`). Lobby and matchmaking support is "experimental, unconfirmed" until tested.

### Game feasibility matrix (hypotheses, nothing confirmed)

| Game | Direct connect via tunnel | Server browser / lobby via tunnel | Required transports |
|---|---|---|---|
| Terraria | Hypothesis: works | n/a (direct IP) | TCP (UDP unresolved) |
| Minecraft Java | Hypothesis: works | Hypothesis: manual add works | TCP; UDP only for query |
| Minecraft Bedrock | Hypothesis: works if UDP relayed faithfully, for Windows/Android/iOS-class clients only | LAN discovery and friends list unlikely [X]; **console and mobile confirmed unable to connect by address** [V] | UDP |
| Left 4 Dead 2 | Hypothesis: works via `connect` | Unknown [X] | UDP (+TCP for RCON, optional) |
| Left 4 Dead | Hypothesis: works via `connect` | Unknown [X] | UDP (TCP unclear) |

**Required experiments:** E-1 Terraria TCP-only join; E-2 Bedrock join through a UDP relay and behavior with non-default ports; E-3 Source engine direct-connect through a relay; E-4 L4D2 lobby join through a relay; E-5 L4D (1) port and protocol capture.

**Still `[R]` after T-002 progress, 2026-10-17:** Terraria's TCP-vs-UDP question (E-1 — its source contradicts itself on a single page); all Valve/Source port specifics for L4D and L4D2 (Valve's pages were not read in this session); and the Bedrock `allow-list` default conflict. **Minecraft Java and Bedrock port facts are now `[V]`.**

### §2.1 Server defaults that change our security posture

Added 2026-10-17 from the default `server.properties` files actually shipped with Minecraft Java Edition 26.3 and BDS 1.26.52.3, and the Terraria Wiki server page. These matter because a tunnel makes a server internet-reachable, which is exactly the situation several of these defaults are *not* safe for.

**Minecraft Java Edition — defaults we must override in generated configuration**

| Key | Default | Why it matters here |
|---|---|---|
| `enable-rcon` | `false` | Correct. Leave off. The wiki's own text warns RCON is unencrypted and should only be used from localhost — independent support for ADR-015. |
| `enable-query` | `false` | Correct. Leave off (GAME-008). |
| `enable-jmx-monitoring` | `false` | **Exposes a JMX MBean. Must stay off.** A remote management surface we do not need and do not control. |
| `prevent-proxy-connections` | `false` | Kicks players when the connecting IP differs from the one that reached Mojang's auth server. **Off by default.** Worth considering ON for a proxied server, but it interacts with our relay and needs an experiment, not an assumption. |
| `online-mode` | `true` | Keep true. |
| `white-list` | **`true`** | Java edition defaults the whitelist **on**. Note this differs from a widely repeated assumption that it is off. |
| `rate-limit` | `0` (disabled) | Packet rate limiting is off by default; relevant to NET-006 and abuse. |
| `max-tick-time` | `60000` | Watchdog kills the server after a 60 s tick and **exits with code 1** — useful for supervision (a crash is distinguishable, T-025/E-17). |
| `server-ip` | blank | Blank means listen on all interfaces. Correct for us. |
| `enable-status` | `true` | Server appears "online" in the server list. |

**Minecraft Bedrock — the transport default changes the port story**

- **`transport=nethernet` is the shipped default**, not `raknet`. Both `server-udp-ports` and `server-ip` are documented as "ignored when `transport=raknet`", and `server-portv6` is "ignored when `transport=nethernet` and a dual-stack socket will be opened on `server-port` instead".
- **Consequence:** the `enable-lan-visibility` behaviour that binds default ports 19132/19133 despite non-default `server-port` is documented as happening "**when `transport=raknet`**". Since the default is `nethernet`, **that port-shadowing may not affect us at all** — which would make GAME-003's non-default-port requirement much easier than assumed. **[X] Must be measured (E-16), not assumed.**
- **Source conflict to resolve:** Microsoft's BDS properties reference states `allow-list` defaults to **`false`**; the `server.properties` shipped in the server states **`allow-list=true`**. Both read 2026-10-17, both community- or vendor-adjacent, and they disagree. **[X] E-16 must determine the effective default.**
- `server-udp-ports` supports `[ip:]external[-external]:internal[-internal]` mappings — an internal-to-external UDP port mapping feature. **Security-relevant and outside our needs.** A generated configuration must not enable it, and this deserves an explicit threat-model row.

**Terraria — the server will try to port-forward itself**

- `serverconfig.txt` documents **`upnp=1`**, and the wiki states that on launch the server "will ask if you want to auto port forward". The server can also auto-forward the port via uPNP.
- **This is the exact behaviour this product exists to avoid.** A host whose manually-configured Terraria server has uPNP enabled has already opened a hole in their router, independently of the tunnel. Provisioning must set `upnp=0`, and the product should *warn* a host whose own Terraria server has uPNP on, because their tunnel address may not be the only way in. **New threat-model row.**
- Terraria's console supports `playing` (player list), `version`, `port`, `save`, `kick`, `ban`, `password`, `motd`, `exit`. So **`playing` is a second possible source of confirmed presence** — console-driven, so it would need process I/O rather than a network API. **[X] not evaluated.**
- Official standalone download exists: `https://terraria.org/api/download/pc-dedicated-server/terraria-server-<version>.zip` (e.g. `...-1458.zip` for 1.4.5.8). On Windows the server normally ships **inside the Steam installation**, so a Windows host may already have it without any download.
- Server needs **.NET Framework 4.0 and XNA Framework** on Windows — a runtime dependency comparable to the JRE question for Java.

**Cross-game note.** Both Minecraft editions state that `server.properties` is **regenerated on startup**, with missing or invalid keys reset to defaults, and that **changes require a restart**. Terraria's config is read via `-config`. So no game in the first cut supports a live configuration apply; the engine must always plan stop → write → start.

### §2.2 Source-engine games identify players by IP — this breaks under a relay

**The most consequential finding of T-002. Read this before designing the relay.**

Source: the Left4DevOps `l4d2-docker` README (read 2026-10-17, community, actively maintained, and it builds images for **both** Left 4 Dead and Left 4 Dead 2). This is `[R]` — Valve's own wiki was **unreachable this session** (see §9) — but the claim is specific, operationally plausible, and testable.

What the project documents:

- Host networking is "the preferred networking method, as it allows the game server to **correctly identify the IP of connecting players** and can be used to ban players or **anyone on the net attempting to force access to RCON**".
- Bridge networking is explicitly the mode where "the game server won't be able to correctly identify player's IP correctly".
- Under bridge networking the game port must be published on **both TCP and UDP** (`-p 27015:27015/tcp -p 27015:27015/udp`), confirming game traffic uses both protocols.
- **"Game traffic and RCON always operate on the same port number"** — 27015 by default, differing only by protocol. Confirms the `research.md` §2 RQ-1 conclusion about TCP and UDP sharing 27015.
- A third management surface exists: **NET_CON_PORT**, a plaintext **telnet** network console (examples use 17017), separate from RCON. The README warns that without a password you should firewall it.
- RCON is an in-game feature (logging in grants vote-kick immunity), and `srcds_run` bans an IP after a few failed attempts — with the warning that under bridge networking this "can have the unintended side effect of banning everyone".

**The conflict.** Our relay accepts a guest connection and then opens a *new* local socket from the engine to the game server (`contracts.md` §8.2, §11). The game server therefore sees every guest arriving from **the same host-local address** — for a default profile, `127.0.0.1`. Consequences:

1. **The game server cannot tell guests apart by IP.** Any game-level IP ban is useless, because there is no per-guest address to ban.
2. **The game's own RCON brute-force protection becomes a liability.** "Ban an IP after a few failed attempts" would ban our own loopback address and lock out the host — exactly the failure the project warns about under bridge networking.
3. **NET_CON_PORT cannot be protected by source IP**, since every connection appears local.
4. **Joining is probably unaffected**, because `connect [ip:port]` identifies a *server*, not a client, and the player is identified by their Steam account. But this is an assumption, not a result — **[X] E-3/E-4/E-5 must confirm it.**

**Candidate mitigations, none chosen [A]:** have the engine bind a distinct loopback source address (e.g. `127.0.0.2`, `127.0.0.3`, …) per guest flow so each appears as a separate client; or accept the collapse and state plainly in the docs that game-level IP banning does not work through a tunnel.

#### §2.2.1 The loopback-per-guest mitigation was tested at the OS level and works (2026-10-17)

The second mitigation above was the highest-priority open architecture issue (`architecture.md` §20 item 13, "must be settled before the engine is designed"), and it was **completely unexamined** — nobody had checked whether Windows even permits the trick. It was testable without a game server, so it was tested.

**Method.** Three Python probes against a loopback listener, run on this development machine (Windows 11). No elevation, no game server, no network access.

**Results, all `[V]`:**

| Probe | Result |
|---|---|
| TCP: bind a distinct loopback source (`127.0.0.2`, `127.0.0.5`, `127.1.2.3`) and connect to `127.0.0.1` | **Succeeds.** The listener observed **four distinct source addresses**, one per flow. No `WSAEADDRNOTAVAIL` |
| UDP: same, datagram source addresses | **Succeeds.** `127.0.0.1`, `127.0.0.2`, `127.0.0.9`, `127.255.255.254` all preserved at the receiver |
| Concurrency: 64 simultaneous flows, one distinct source each | **64 accepted, 64 distinct source IPs observed, 0 collapsed**, 0 errors, 0.01 s |
| Address range | `127.0.0.0` and `127.255.255.255` are **rejected** (`WSAEADDRNOTAVAIL`, WinError 10049); `127.0.1.1`, `127.1.255.255`, `127.128.0.1`, `127.255.255.254` all bind. `128.0.0.1` and `10.0.0.1` are correctly rejected |
| IPv6 | Only `::1` binds. **`::2` is rejected.** An IPv6-only design would get no per-guest separation — the engine's loopback-source scheme must be IPv4 |
| Privilege | **None required.** No admin rights, matching the project's no-elevation constraint |

**What this settles.** The mechanism `architecture.md` §20 item 13 proposed **works at the operating-system level**, for both TCP and UDP, at concurrency, with no elevation, and with ~16 million usable source addresses. The idea was sound and nobody had checked it. The engine can allocate `127.0.x.y` per guest flow from a pool that must **exclude `.0.0` and `.255.255.255`**.

**What this does *not* settle — the experiment stops where the game server begins.** These probes prove Windows preserves the source address. They do **not** prove:

1. **That a Source engine server accepts and honours it.** Whether `srcds_run` treats `127.0.0.2` as a distinct player, and whether its own bind/`sv_lan` logic accepts connections from loopback sources other than `127.0.0.1`, is untested. **This is the load-bearing unknown and it needs a real server: E-3, E-4, E-5.**
2. **That the RCON brute-force ban becomes safe.** It would now ban *one* guest's address rather than the host's — plausibly correct, but only a real failed-login test confirms it, and the consequence of getting it wrong (host locked out of its own server) is severe.
3. **That Source supports IPv6 at all.** Given `::2` does not even bind, a per-guest loopback scheme is IPv4-only, and an IPv6-preferring game server would need explicit configuration.

**[A] Design consequence, not a decision.** If E-3/E-4/E-5 confirm item 1, the engine needs a **loopback source pool** allocator with explicit exclusion of `.0.0` and `.255.255.255`, and the relay needs to hand each guest flow a stable address for the life of that flow (reusing an address while it is live would recreate the collapse). No schema change is implied — this is engine and relay data-path state, which `architecture.md` §20 item 8 already places outside the database.

**Confidence.** `[V]` for the OS behaviour, measured and reproducible on this machine. `[A]` for everything downstream of it. The distinction matters: this result is stronger than anything else in §2.2 and it still does not license a claim about Source-engine behaviour.

**This also reaches Minecraft.** Java edition's `prevent-proxy-connections` kicks a player when the connecting IP differs from the one that reached Mojang's authentication server. Behind our relay the server sees loopback while the player authenticated from their real address — a guaranteed mismatch. That setting **must stay `false`** (it is `false` by default) and the generated configuration should say why. Bedrock behaves similarly: Xbox Live authentication happens client-side, but the address seen by the server is ours.

**Consequence for guest admission (E-13).** Our relay-side source-IP admission is unaffected — it operates on the *public* connection before the engine dials. But the host must be told that the game's own view of player addresses is lost. Recorded as `AS-12` and as an open architecture issue; it does **not** by itself invalidate E-13.

### §2.3 Terraria's UDP question: what the sources actually say

Searched 2026-10-17. Sources split, and the split is instructive rather than random:

- **Terraria Wiki (official wiki):** "The server runs on **TCP** port 7777 by default" — but the same page tells cloud operators to "ensure port 7777 on **TCP and UDP** is open".
- **Official Terraria forums port-forwarding guide:** "Activate both **TCP/UDP** protocols for the port" — advice about what to forward, not a statement about what the server listens on.
- **Two hosting vendors contradict each other.** One states "The game traffic runs over exactly one port, 7777 TCP, and the game opens **no UDP port at all**"; another claims Terraria "uses both TCP and UDP on that port".
- A community answer asserts Terraria "does not (and never did) use UDP".

**Reading [A]:** the TCP-and-UDP mentions are most likely **defensive port-forwarding advice** — forward both so you never miss the one you need — rather than a claim that a UDP listener exists. On that reading, TCP 7777 alone is correct and `AS-7` holds. But this is inference from the *shape* of the sources, not a measurement, and one vendor asserts the opposite.

**E-1 becomes cheap and decisive:** start a Terraria server and observe which sockets it actually listens on (`netstat -anb`, or the engine's own probe). That answers the question outright rather than by argument. Recording as `[X]`; do not promote `AS-7` to verified on the strength of this section.

## 3. TCP vs UDP forwarding and UDP-over-tunnel transport

### RQ-3: How should UDP be carried through a tunnel to a relay?

Alternatives examined: QUIC with DATAGRAM frames, UDP encapsulation in HTTP (CONNECT-UDP), WireGuard-style UDP tunnel, TCP-only encapsulation.

Evidence:
- RFC 9221 adds an unreliable DATAGRAM frame to QUIC; RFC 9298 defines proxying UDP in HTTP; RFC 9297 defines HTTP Datagrams and the Capsule Protocol. I confirmed the RFC titles and roles from secondary summaries and an RFC XML mirror, **not** the RFC text itself [R].
- A QUIC datagram must fit in one QUIC packet, so path MTU limits payload size [R, Cloudflare-style explainer]. [A] Game UDP packets are usually small, but this must be measured against real Bedrock and Source traffic. [X]
- CONNECT-UDP is a *proxy* protocol (client asks a proxy to send UDP to a target host and port). [A] For this product the relay should **not** open sockets to client-chosen targets, which is the SSRF-like risk the prompt warns about. Borrowing the datagram mechanism (QUIC datagrams) is plausible; adopting CONNECT-UDP's client-chosen target model is not.

Recommendation (tentative): **QUIC with DATAGRAM frames for UDP and QUIC streams for TCP**, multiplexed on one authenticated connection per tunnel, with allocation done by the control plane. Confidence: **low to medium** until prototype E-6 (UDP datagram round trip through a QUIC relay under loss/jitter) and library evaluation are done.

TCP-only encapsulation is rejected for UDP games [A]: head-of-line blocking and retransmission would add latency to loss-tolerant game traffic.

## 4. NAT traversal and relay fallback

### RQ-4: Relay-first, direct P2P, or hybrid?

Evidence:
- Tailscale's own engineering blog says direct NAT traversal success is "well north of 90%" in typical conditions by their internal metrics, uses relays (DERP) to initiate connections and as fallback, and names symmetric NAT and stacked NATs as hard cases [V as to Tailscale's claim; it is vendor-reported and specific to their user base, not a general rate].
- Third-party summaries state relayed traffic is the fallback for symmetric NAT, carrier-grade NAT, and UDP-blocking firewalls, and that the relay sees metadata but not WireGuard payload [R].
- A Go library README states symmetric-NAT peers stay on relay permanently [R].

Findings:
1. A relay is needed regardless; the hybrid pattern (start on relay, upgrade to direct) is established practice [V/R].
2. The ">90%" figure must not be used as this product's expected rate. Users here are game players on home, mobile, and school networks. [A]
3. A relay-first MVP is the simplest architecture that works on every network. Direct upgrade is an optimization, not a launch requirement. [A]

Recommendation: **ADR-002, relay-first MVP, with direct connect deferred behind experiment E-7** (measure direct-path success rate and latency gain on real networks in the target audience's regions). Confidence: medium.

## 5. Desktop, engine, packaging, hosting, identity, backend, observability, and testing research

### RQ-5: Which desktop stack and process model?

Alternatives: WPF, WinUI 3, Avalonia/.NET (C#); Tauri 2 (Rust + system WebView); Electron (bundled Chromium + Node).

Evidence quality is **poor**. I found blog and aggregator comparisons whose numbers conflict (different Tauri and Electron versions, no shared methodology), so **no size, memory, or startup figures are used here**. What the sources agree on [R]: Tauri uses the OS web view (WebView2 on Windows) instead of bundling a browser; Electron bundles Chromium and Node.js; Electron has the larger ecosystem and tooling.

Reasoned analysis [A]:
- The tunnel engine needs QUIC with datagrams (RQ-6). Rust has a maintained library with datagram support; for .NET I could not confirm one (RQ-6). That favors a **Rust engine** regardless of the UI choice.
- Tauri puts the UI shell and engine in one language and toolchain. WPF/WinUI would add a second runtime and an IPC boundary (engine as a sidecar process).
- WPF remains a sound Windows-only option with strong native accessibility and UI-automation support. It is not rejected on technical grounds; the case against it is the extra boundary and weaker cross-platform path.
- Process model: **engine in-process or as a child process of the app, running as the current user.** A Windows service would need admin rights to install, and nothing in the requirements so far needs one. Revisit only if an experiment shows a need (for example, running while logged out).

Recommendation (ADR-005, proposed): **Tauri 2 shell + Rust engine crate**, with a WPF-plus-Rust-sidecar fallback. Confidence: **low to medium**. Experiment E-8: build a thin shell and a loopback engine, then measure startup, idle memory, and accessibility behavior ourselves.

### RQ-6: Which QUIC library for the engine and relay?

- Quinn's API exposes `send_datagram` and datagram reads on a connection, as shown in third-party crate source that wraps it [R]. A Quinn fork ("noq") lists RFC 9221 support and, separately, *draft* specifications for QUIC multipath, address discovery, and NAT traversal [R]. Those drafts are not standards and must not be treated as stable.
- For .NET, I found only a design proposal for `System.Net.Quic` datagrams, which says the underlying MsQuic library supports the extension. I could **not** confirm that a datagram API has shipped. [X]
- One source says quiche lacked datagram support, but it is an old proposal text and I discount it.

Recommendation: Rust + Quinn for client engine and relay. Confidence: medium on datagram availability, low on production fitness until E-6. Maintenance note: pin versions, track Quinn's release cadence, and keep the transport behind an internal interface so it can be swapped.

### RQ-7: Packaging and updates without administrator rights

- Microsoft's MSIX documentation distinguishes per-user registration from all-users provisioning [V].
- Velopack installs per user under `%LOCALAPPDATA%` with built-in auto-update, per two open-source projects that use it [R].
- Tauri's updater verifies a signature on the downloaded artifact, supports NSIS and MSI on Windows, and has a quiet install mode that only works without admin for per-user installs [R, from crate source and v1 docs]. Its config also has options named "dangerously" for insecure transport and invalid certificates, which must stay off [R].
- A Tauri app needs WebView2, which a community installer README says ships with Windows 10 20H2 and later [R].

Recommendation (ADR-006, proposed): per-user NSIS installer plus the Tauri updater with signed artifacts; MSIX/Store distribution deferred. Confidence: low to medium. Experiment E-9: full update, failed-update, and rollback test on a clean Windows VM, including a tampered-artifact rejection check.

### RQ-8: What drives relay hosting cost?

- Outbound transfer is the dominant variable cost for a relay [A]. A relay forwards every game packet to the other peer, so billed egress tracks total game traffic. I have **no measured bytes-per-session for any of the five games**, so I give no cost estimate. [X]
- Included transfer and overage differ widely across VPS providers in comparison articles: Hetzner roughly 20 TB per server in EU locations at about €1/TB over, Vultr and DigitalOcean low single-digit TB at $0.01/GB over [R]. One article says Hetzner's US allowance is only 1 TB, and one says Hetzner has no APAC locations. Sources conflict and prices change, so **every number here must be re-checked on the providers' own pricing pages before use**.
- Region choice affects both latency and allowance, and the target player regions are not yet decided. [A]

Recommendation: no provider decision yet. Experiment E-10: capture bytes per player-minute for each game under realistic play, then model cost per active hour per provider.

### RQ-9: Authentication and credential storage

- RFC 8252 (BCP 212) says OAuth authorization requests from native apps should only go through external user-agents, primarily the user's browser [V, RFC Editor]. [A] Consequence: sign-in must open the **system browser**, not an embedded web view, which matters because the Tauri shell itself is a web view.
- The OAuth device authorization grant (RFC 8628) is the usual alternative when a browser redirect is awkward. I read only a vendor's description of it, not the RFC text [R]. That description lists device-code phishing as a risk and uses short pickup windows and single-use tokens as mitigations [R].
- For storing tokens on Windows, one vendor doc recommends Windows Credential Manager [R], and an open-source Windows tool encrypts its OAuth refresh token with DPAPI under the current user [R]. [A] Either way, secrets must never go into plain config files or logs.
- **Not researched:** whether to run our own accounts or delegate to an external identity provider (OIDC), passkey support, and how a guest without an account would receive a scoped, revocable invitation credential. That last point depends on ADR-007.

Recommendation (ADR-009, proposed for the flow; provider open): system-browser authorization-code flow with PKCE per RFC 8252, refresh token in the OS credential store, short-lived access tokens. Confidence: medium on the flow, none yet on the provider. Experiment E-12: spike sign-in with loopback redirect (and device flow as fallback) and credential-store round trip.

### RQ-10: Backend framework and database

**Updated 2026-10-17.** This section previously carried no external evidence at all — every claim was reasoning, and ADR-008 was logged at *low* confidence because of that. Candidates surveyed: a Rust (axum-style) service, TypeScript on Node, C# on ASP.NET, Go.

Evidence now gathered (2026-10-17), all `[V]` for existence and maintenance status, **not** for fitness:

| Candidate | Latest | Last release | Licence | Repo activity | Notes |
|---|---|---|---|---|---|
| `axum` | 0.8.9 | 2026-04-14 | MIT | pushed 2026-10-15, not archived, 27.5k stars | Actively maintained |
| `tokio` | 1.53.3 | 2026-10-13 | MIT | pushed 2026-10-16, not archived | Actively maintained |
| `sqlx` | 0.9.0 | 2026-05-21 | Apache-2.0 | pushed 2026-10-16, not archived | See RQ-14; **145 transitive crates**, measured |
| `serde` / `serde_json` | 1.0.229 / 1.0.154 | 2026-07-18 / 2026-10-11 | MIT/Apache-2.0 | — | Both licences permitted by `deny.toml` |
| `tracing` | 0.1.44 | 2025-12-18 | MIT | — | Matches ADR-010 |

**What this evidence does and does not establish.** It shows the Rust stack is alive, permissively licensed, and compatible with the existing `deny.toml` allowlist. It does **not** show that the stack is *right for this project* — that turns on team familiarity and on `quinn` (ADR-001/RQ-6), which is a separate dependency and a separate risk. No TypeScript, C#, or Go option has been measured or costed, so this table cannot be read as a head-to-head comparison.

Reasoned recommendation (ADR-008): Rust control plane sharing the protocol crate with the engine and relay, PostgreSQL for durable state, relay session state in memory only. Confidence moves **low → medium** on the strength of the maintenance and licensing evidence, and **not higher**, because the decisive input — team familiarity — has still not been supplied by the owner.

### RQ-11: Logging and observability

- OpenTelemetry's Rust implementation reports different maturity depending on the page. Older copies of the README say logs and metrics are alpha and traces beta [R]. A recent guide and a newer README copy say the logs and metrics APIs and SDKs are stable while traces are still beta or release-candidate [R]. Treat **traces as beta** and confirm the current status table on the OpenTelemetry site before depending on it.
- The `tracing` crate bridges to OpenTelemetry through maintained appender crates [R], and exporters exist for OTLP and Prometheus [R].

Recommendation (ADR-010, proposed): structured logging with `tracing` in all Rust components; OTLP export for metrics and traces; pin versions; keep exporters optional so the desktop client sends **no telemetry by default** (see privacy requirements). Confidence: medium. Retention, log redaction rules, and alert thresholds are still to be defined.

### RQ-12: Testing and game-server integration

- The widely used `itzg` Docker images run Minecraft Java and Bedrock servers, and bundle `mc-monitor`, which checks server status for Java (`status`) and Bedrock (`status-bedrock`) [R, project repos].
- A community Terraria image exists whose health check is a plain TCP connect to the port [R]. L4D2's dedicated server is a free anonymous SteamCMD download (app 222860) per community guides [R].
- **Limit:** a status ping or TCP connect proves the server answered, not that a real game client can join and play. The prompt requires real-join verification.

Proposed compatibility test levels (ADR-011, proposed):

| Level | Proves | Automation |
|---|---|---|
| L0 | Port reachable through the tunnel (TCP connect or UDP echo) | Fully automated |
| L1 | Game protocol answers through the tunnel (e.g. `mc-monitor`) | Automated where a tool exists (Minecraft); needs custom probes for Terraria and Source |
| L2 | A real game client joins and plays | Manual until E-11 shows automation is feasible |

A game is not marked "supported" until L2 passes for it. Experiment E-11: determine whether any headless or scriptable client exists for each game.

### RQ-14: Which migration tool should own `migrations/`?

Added 2026-10-17 for T-003. Survey only — **no decision is recorded and none should be until ADR-008 is settled**, because every candidate implies a database driver and ADR-008's confidence is *low* (`research.md` §5 RQ-10: "reasoning only, no evidence gathered"). Choosing a migration tool now would quietly endorse a driver that has never been justified.

Constraints the tool must satisfy, from `schema.md` and `data-model.md` §8 rather than preference:

- Forward-only, one migration step per deploy, applied before new code starts.
- **Never edit an applied migration.** Rollback is a compensating forward migration.
- Expand/contract: add nullable/defaulted columns first, backfill, switch reads, drop later.
- Indexes built concurrently where supported.
- CI applies every migration to an empty database and checks the result.
- `schema-draft.sql` must be deleted in the same change that adds the first migration.

Candidates

| Candidate | Evidence read 2026-10-17 | Notes |
|---|---|---|
| **`sqlx-cli`** (`sqlx migrate`) | crates.io: latest **0.9.0**, published **2026-05-21**; licence `MIT OR Apache-2.0` per its own `sqlx-cli/Cargo.toml`; repo `launchbadge/sqlx`; workspace `rust-version = "1.94.0"`, edition 2021; has a `migrate` feature and reversible `.up.sql`/`.down.sql` pairs | Actively maintained and the default choice in Rust/PostgreSQL circles. **Cost: 145 transitive crates** for the CLI, measured here. Doubles as the query layer, so one dependency instead of two. Its MSRV (1.94) is above this repo's declared 1.77, which needs a decision |
| **`refinery`** | **Not examined in depth.** Checked 2026-10-17 for prebuilt Windows binaries and GitHub releases: **none found**, so it would have to be compiled from source. Would also be a *second* migration dependency when the driver is already `sqlx` | A reasonable alternative, but the driver-plus-two-tools cost is the main argument against it |
| **Hand-rolled numbered `.sql` files** | Already described in `schema.md` | Zero dependencies, fully under our control, and the documented workflow is already tool-neutral. But the bookkeeping (ordering, checksums, "applied" tracking) becomes ours to get right, and `data-model.md` §8 forbids editing history, which a naive runner may not enforce |

**[A] Reasoning, not evidence:** given `SEC-017` (CI must fail on a known-vulnerable dependency) and a still-empty dependency graph, a zero-dependency option has a real advantage, but hand-rolling migration bookkeeping is exactly where subtle corruption bugs live. Do not pick the smallest dependency count as a tiebreaker without the ADR-008 decision first.

**Resolved 2026-10-17 — `sqlx migrate` chosen, see ADR-017.** The MSRV question is moot: ADR-008 already raised the workspace to 1.94. The 145-crate cost was measured and accepted as part of ADR-008.

**What is still unverified about the chosen tool:** `sqlx-cli` **could not be executed in this environment.** Compiling it fails on the GNU toolchain because `dlltool.exe` is absent, and the MSVC toolchain has no Windows SDK; `sqlx` publishes no prebuilt Windows binaries, so there is no fallback. T-033 must therefore **verify migration behaviour rather than assume it** — specifically that statement boundaries are parsed correctly, which is the exact defect class that broke the first schema apply (see `schema.md`).

### RQ-13: Can the app safely provision and supervise a game server on the host's own PC?

Added 2026-10-17 for the scope change in `context.md` G-7. Scope agreed for the first cut: **host-local only, Minecraft Java and Minecraft Bedrock.** Operator-hosted servers are out of scope (NG-1).

**Status: partially answered 2026-10-17 (desk research only).** E-15 and E-16 are **not** complete — their "Done when" conditions require running on a clean standard-user VM, which has not happened. Nothing below has been executed; every statement is a documentation reading. No experiment was run and no server was started.

#### RQ-13.1 Sources actually read (all accessed 2026-10-17)

| Source | Type |
|---|---|
| Microsoft Learn, "Getting Started with Bedrock Dedicated Server" (`ms.date` 2025-05-30, updated 2026-01-28) | **Primary (vendor)** |
| Microsoft Learn, "Bedrock Dedicated Server Properties" (`ms.date` 2025-06-02) | **Primary (vendor)** |
| Minecraft Wiki, "Bedrock Dedicated Server" | Community |
| Minecraft Wiki, "server.jar" | Community |
| Minecraft Wiki, "Server/Requirements" | Community |
| Minecraft Wiki, "Version_manifest.json" | Community |

#### RQ-13.1b The EULA and Usage Guidelines were read on 2026-10-17 — hosting is permitted, and six constraints are now binding

**The blocker is cleared.** After repeated timeouts through the `webfetch` path, DNS and TCP 443 were confirmed reachable for `aka.ms`, `www.minecraft.net`, and `web.archive.org`, and a direct `Invoke-WebRequest` succeeded: `aka.ms/MinecraftEULA` → **HTTP 200, 35,444 bytes**, redirecting to `https://www.minecraft.net/en-us/eula`; `https://www.minecraft.net/en-us/usage-guidelines` → **HTTP 200, 407,376 bytes**. Both were read in full and the quotes below are verbatim. The earlier failures were a transport problem with one fetch path, not the sites being unavailable — worth recording, because "the site is down" and "my client cannot reach the site" are different claims and only one of them was ever true.

**This is not legal advice.** What follows is what the documents say, quoted. Whether our specific implementation complies is a judgement the owner or their counsel must make, and `AS-11` is marked accordingly rather than simply ticked.

**Permission is explicit, in three places. [V]**

| Source | Quote |
|---|---|
| Usage Guidelines, "Servers and hosting" | "**You may host a server for Minecraft. You may even charge for access to the server** as long as you follow the guidelines in this section. By 'server,' we mean a single connecting address or IP number." |
| EULA, "what you can and can't do with Minecraft software and content" | "For the server version of Minecraft: **Java Edition, you can install it on a server and host online play.**" |
| EULA, Summary | "You may **develop tools, plug-ins and services** as long as they do not seem official or approved by us, such as by using our logos." |

That last one is the clause that covers this entire product: a desktop application that provisions and tunnels Minecraft servers is a "service", and it is permitted provided it does not appear official.

**"By 'server,' we mean a single connecting address or IP number" is load-bearing.** The product's core value is that a guest reaches the game through *our* relay address rather than the host's own IP. The Guidelines anticipate this and still permit it, because the unit of counting is the connecting address. **But it means per-connection-address obligations attach to us, not only to the host** — and one of them is privacy, below.

**Six constraints are now binding engineering requirements, not background reading. [V] for the text; [A] for how each maps to our design.**

| # | Constraint (verbatim) | What it requires of us |
|---|---|---|
| **C1** | Access to your server "**Must only be granted to users who have a genuine paid-for version of Minecraft**" | Guest admission must be gated to genuine purchasers. **Mojang publishes no API a third party can call to verify this.** See the open question below — this is the one requirement with no clean implementation |
| **C2** | Access "**Can't be limited to or controlled, directly or indirectly, by a player owning or having access to out-of-game content, products, or services**" | **This puts ADR-007's guest model at risk.** If a guest must hold an account on our platform to obtain the relay address, that is out-of-game gating. Guest model **G2 (connect to the relay address, no client, no account)** is compatible; **G1 (guest installs our client and signs in) appears to conflict.** ADR-007 is open; this evidence bears directly on it and must be read before G1 is chosen |
| **C3** | "You **don't hide or alter any of the dialogs or prompts** that are part of Minecraft (this includes the **End User License Agreement (EULA)**, warning messages, and error messages)" | Provisioning must not touch `eula.txt` or any Minecraft prompt. This **confirms** the design decision already recorded in T-027: the app never accepts the EULA on the host's behalf; the host does it deliberately |
| **C4** | "You make it clear that **you are responsible for all end user data privacy and protection requirements**" | The **operator** carries end-user data privacy. This sharpens PRIV-002 and PRIV-003: guest source IPs are the operator's liability, not merely a data-minimisation choice. E-13's viability now has a legal dimension, not just a technical one |
| **C5** | "You provide information to users on **all the content and pricing** applicable to your server **before a person joins**, signs up, or signs in" | Only bites if we monetise. Recorded so it is not discovered after launch |
| **C6** | Monetisation conditions: equal cost for everyone; every payer gets all enabled mods; own and control the server while charging; **no entitlements that give a competitive advantage**; cosmetics only, **"except for capes"**; virtual currency with no real-world value | Any future paid tier must not sell game advantage. Directly constrains quota design: **a quota that lets a paying host admit more players is competitive advantage**, and would need to be structured so it does not |

**Plus a branding requirement that is concrete and easy to miss. [V]** The Usage Guidelines' Essential guidelines require, prominently: **"NOT AN OFFICIAL MINECRAFT [PRODUCT/SERVICE/EVENT/etc.]. NOT APPROVED BY OR ASSOCIATED WITH MOJANG OR MICROSOFT"** — on the product, listing, description, website, and all other related materials. They also require naming the publisher/owner and giving a contact method, and state that **"chat and forum links are not acceptable methods"**. That is a specific deliverable for `DESIGN.md` and the about screen, and it is currently in no requirement. Added as `SRV-012`.

**What the documents do *not* resolve — recorded rather than guessed.**

1. **C1 has no published verification mechanism.** No Mojang/Microsoft API is offered to third parties for confirming a player holds a genuine paid-for copy. In practice the host's own account and the guest's own client are the only evidence. **[X] This is an open design question, not a solved one.** A reasonable reading is that the obligation sits on the party running the server, so surfacing the rule to the host is the honest implementation — but that is `[A]`, and the owner should confirm it.
2. **C2's exact reach is a judgement.** Whether "out-of-game content, products, or services" covers *any* account on the operator's platform, or only paid entitlements, is not spelled out. The safer reading is the strict one, and it happens to coincide with the design we already prefer (G2). Recorded as a constraint on ADR-007, not as a resolution of it.
3. **Whether the operator can pass C4 to the host.** Our terms of service would need to allocate end-user data privacy explicitly. No document here governs our own relationship with the host, and that is ours to write.

**Effect on the assumptions.** `AS-11` moves from `[X] unverified` to **answered by primary source, with a legal sign-off still outstanding** — the text permits what we do and imposes six constraints we had not captured. `E-19`'s licensing half is answered; its standard-user VM footprint half is **not** and still needs a real VM. `plan.md`'s stated risk that "game servers cannot be obtained or run legally" is **retired for Minecraft Java**: the remaining acquisition questions are technical (E-15), not licensing.

#### RQ-13.2 Java Edition (`server.jar`)

- [V] Distributed by Mojang Studios, free of charge, licensed under the EULA linked from `minecraft.net/en-us/eula` — **now read in full; see RQ-13.1b**.
- [V] Object store is **content-addressed by SHA-1**: observed download path form is `piston-data.mojang.com/v1/objects/<sha1>/server.jar`, e.g. release `26.3` at `.../objects/33680f5f2ac32864d6d7cf5e56a705fdb3e05f4c/server.jar`.
- [V] `version_manifest_v2.json` at `piston-meta.mojang.com` carries a **`sha1` field per version entry** (documented field list). HTTPS throughout.
- [X] **Not confirmed:** that the manifest was actually fetched and that a manifest-sourced SHA-1 verifies for the `server.jar` object specifically (the manifest hashes the version JSON, not necessarily the server jar). **E-15 must verify the real check end to end before `SRV-002` can be claimed.**
- [V] **A JRE is required, and the requirement moves with game versions** — JRE 8 historically, 16 from 1.17, 17 from 1.18, **21 from 1.20.5**. The page I read states requirements for `26.3` but does not restate the minimum, so the exact floor for the current release is [X]. **Now measured: `javaVersion.majorVersion` is `25` for `26.3` (RQ-13.2b), so the floor question is answered per-version and answerable at download time rather than hard-coded.**
- [A] **Consequence:** provisioning Java edition means acquiring *two* artifacts — the server jar **and a JRE** — from two different vendors, both verified, neither shipped in our installer. This roughly doubles the supply-chain surface of `SRV-002` and must be budgeted in `SRV-001` scope and disk footprint.
- [V] Legal acceptance is an explicit user act: `eula.txt` is generated with `eula=false`, the process exits, and `eula` must be set to `true` to start. **Confirmed as a design constraint by RQ-13.1b C3 and `SRV-015`: the app must never do this for the host.**
- [V] `--initSettings` creates `server.properties` and `eula.txt` **without starting the server** — a clean hook for provisioning.
- [V] Generated files: `server.properties`, `ops.json`, `whitelist.json`, `banned-players.json`, `banned-ips.json`, `logs/latest.log` plus gzipped rotations, world directory named by `level-name`, `libraries/`, `versions/`. **Superseded in part by RQ-13.2b: the shipped artifact is now a self-contained bundler jar, so `libraries/` and `versions/` are inside the jar rather than written beside it.**

#### RQ-13.2b The Java Edition integrity chain was verified end to end (2026-10-17)

This closes the `[X]` above and is the evidence `SRV-002` needs. Run live against Mojang's endpoints on 2026-10-17. **Everything here is `[V]` — measured, not read off a documentation page.**

**The chain, and the correction that makes it correct.**

| Step | Result |
|---|---|
| 1. `https://piston-meta.mojang.com/mc/game/version_manifest_v2.json` | HTTP 200, `application/json`, **277,821 bytes**, **919 versions**. Latest release **`26.3`**, released `2026-09-15T11:23:02+00:00`, manifest updated `2026-10-13T06:43:38+00:00` |
| 2. Manifest entry for `26.3` | `url` = `https://piston-meta.mojang.com/v1/packages/11f1aefe473eefba0af62e3286c33b692807b2eb/26.3.json`, and `sha1` = `11f1aefe…` |
| 3. **What that `sha1` actually covers** | **The version JSON, not the server jar.** I downloaded the version JSON and computed SHA-1 over it: `11f1aefe473eefba0af62e3286c33b692807b2eb` — **exact match.** This confirms the caveat I had recorded as `[X]` and is the single most important finding here, because verifying the server jar against the *manifest* hash would **always fail** and an implementer who assumed otherwise would conclude the artifact was tampered |
| 4. The **correct** hash for the artifact | It lives at `downloads.server.sha1` **inside the version JSON**: `33680f5f2ac32864d6d7cf5e56a705fdb3e05f4c`, with `size` `62294556` and the content-addressed URL |
| 5. End-to-end verification | Downloaded **62,294,556 bytes**; streamed SHA-1 = `33680f5f…5f4c` — **exact match.** Byte count matched the declared `size`. **So the working chain is: manifest → version JSON → `downloads.server.sha1` → download → SHA-1 → compare.** `SRV-002` is satisfiable for Java Edition |
| 6. Is there a sidecar checksum? | **No.** `server.jar.sha1` and `server.jar.sha256` both return HTTP 404. The version JSON is the **only** published source of the expected hash, so an implementation cannot skip that fetch |
| 7. Unattended download | **Plain HTTPS GET, no cookies, no authentication, no special headers.** 62 MB in ~1.0 s (~68 MB/s) on this connection. Satisfies the no-elevation and no-account constraints |
| 8. Required runtime | `javaVersion` = `{"component": "java-runtime-epsilon", "majorVersion": 25}` — **Java 25 is required.** This is readable from the version JSON at download time, so the floor never has to be hard-coded |
| 9. Host has Java 25? | **Yes** — `java version "25.0.1" 2025-10-21 LTS`, plus `C:\Program Files\Java\jdk-25`. **[V] for this machine only** — it says nothing about a typical host, which is the real `SRV-001` risk |

**The artifact is a self-contained Mojang Bundler jar, not a plain server jar.** `META-INF/MANIFEST.MF` reads `Main-Class: net.minecraft.bundler.Main` and `Bundler-Format: 1.0`. 165 entries. It **contains its own 40 dependency jars** under `META-INF/libraries/` (Netty 4.2.16, Guava 33.6.0-jre, Log4j 2.26.0, JNA 5.17.0, fastutil 8.5.18, gson 2.14.0, authlib 10.0.77, brigadier 1.3.11, …) and the real server at `META-INF/versions/26.3/server-26.3.jar` (23.3 MB compressed, 26.7 MB uncompressed).

**Two consequences, both good, both worth recording because they change provisioning scope:**

1. **No library resolution is needed at install time.** Historically a server jar fetched its dependencies from a separate endpoint; bundling them means provisioning downloads and verifies **one** artifact. That is a smaller supply-chain surface than the `[A]` note above assumed, and it removes a whole class of "a library 404s at provision time" failure.
2. **No Windows native is bundled.** The Netty native jars present are `linux-aarch_64`, `linux-x86_64`, `osx-aarch_64`, `osx-x86_64` only. On Windows Netty falls back to its pure-Java NIO transport. **[A]** this is the expected and correct behaviour for Windows, not a packaging defect — but it is a performance-relevant fact for `PERF-001`, which must measure the Windows path and cannot assume Linux-class numbers.

**A correction to my own work, recorded because it nearly produced a false finding.** My first download script opened the output file with `"wb"` **inside** the per-chunk loop, truncating it on every iteration. The saved file ended up 428,572 bytes instead of 62 MB, and `zipfile` then reported entries with **negative header offsets** and 7.95 bits/byte of entropy. I briefly concluded the jar was encrypted or obfuscated. **That conclusion was wrong and was an artefact of my own bug, not a property of the artifact.** The SHA-1 had been computed over every streamed byte and was valid throughout. Re-downloaded with a single file handle: first four bytes are `50 4b 03 04` (ZIP magic), 165 entries, all offsets sane. **Lesson recorded rather than quietly fixed: a truncated file that still hashes correctly will silently corrupt every structural conclusion drawn from it, so size must be checked against the manifest before the bytes are interpreted.**

**What E-15 still does not answer, and why.**

1. ~~The server has never been started.~~ **See RQ-13.2c — it has now been started.**
2. **The JRE supply question is open.** `SRV-010` forbids a system-wide install, and Mojang's `java-runtime-epsilon` component name suggests a published runtime, but **I did not find and verify a download endpoint for it** — the `launchermeta.mojang.com` runtime-manifest paths I tried returned 404, and guessing further URLs would be exactly the kind of invention `AGENTS.md` forbids. **[X] open.** Options remain: detect a suitable host JRE and refuse clearly if absent, bundle a redistributable runtime, or document a prerequisite.
3. **Standard-user footprint is unmeasured on a clean VM.** Measured on *this* machine in RQ-13.2c, which is not the same thing: a developer machine already has Java 25 installed, so the JRE-supply problem is masked here exactly as it would be hidden in a naive test.

#### RQ-13.2c The Java server was actually started, and four requirements changed (2026-10-17)

Run on this machine after the owner authorised it. All `[V]` — measured, and the raw output is reproducible. This is the first time any experiment here has executed a game server rather than reading about one.

**Sequence and results.**

| Step | Result |
|---|---|
| `java -jar server.jar --initSettings` | **Exit 0.** Unpacked the bundler contents, wrote `server.properties` (**69 keys**) and `eula.txt` with **`eula=false`**. **Did not start the server.** Confirms the desk-research claim |
| Start **without** accepting | Refused: *"You need to agree to the EULA in order to run the server."* — **and exited with code 0** |
| Accept (`eula=true`), start with `nogui` | World generated, **"Done (1.320s)!"**, listening on **`*:25565`** TCP. Required Java 25, already present |
| `FR-011` L0 probe | Plain TCP connect to **`127.0.0.1:25565`** and **`::1:25565`** both **succeed**. The `::` listener is dual-stack and accepts IPv4 loopback. The server sends **no payload** until a legacy ping, so the probe must be **connect-only or ping-initiated**, never "expect data" |
| §2.2.1 mitigation, real server | **`127.0.0.7 → 127.0.0.1:25565` connected.** The distinct-loopback-source mechanism works against a real Minecraft server, not only against my own listener. **Still needs a Source engine to confirm (E-3/E-4/E-5)** — Minecraft identifies players by authenticated UUID, not IP, so it does not exercise the identity problem that motivated the fix |
| `SRV-010` on this machine | **Not elevated. 0 services, 0 scheduled tasks created.** Nothing written outside the scratch directory |
| Footprint | **125.8 MB, 87 files**: 62 MB `server.jar` + **40.9 MB `libraries/` (39 files)** + **25.4 MB `versions/` (1 file)** + `world/` (26 files) + logs |
| `SRV-004` defaults | `enable-rcon=false`, `enable-query=false`, `online-mode=true`, `rcon.port=25575`, `query.port=25565`, and **`prevent-proxy-connections=false`** — the last one is exactly what §2.2 requires and it is the shipped default, so the generated config must preserve it |

**Correction to RQ-13.2b.** I wrote that the bundler jar is "self-contained … no library resolution is needed at install time." **That is half right and the wrong half is the one that matters for planning.** No *network* resolution happens — correct. But the bundler **unpacks to disk on first run**, creating `libraries/` (41 MB) and `versions/` (25 MB) beside the jar. So **provisioned footprint is ~126 MB per server, not 62 MB**, and a second version means a second `versions/` entry. That number belongs in the disk-budget part of `SRV-001`.

**ADR-015 is now exercised, and one of its sources is wrong.** The server **does** generate its own `management-server-secret` into `server.properties` when the value is empty, exactly as RQ-13.4 predicted — so the app never creates or holds it, which is the whole point of that decision. But RQ-13.4 records the length as *"exactly 40 alphanumeric characters"* from the wiki. **The observed value is 43 characters.** Corrected below. The other predictions held: `management-server-enabled=false`, `-host=localhost`, `-port=0` (random per start), `-tls-enabled=true` with an empty keystore path, and `-allowed-origins=` empty.

**Two findings that change `SRV-007` and were not anticipated anywhere.**

1. **A refused start exits 0.** The EULA refusal is a *successful* exit as far as the OS is concerned. A supervisor that treats exit 0 as "started, then stopped cleanly" would report a healthy start that never happened, and a naive bounded-restart loop would burn all its retries on a condition no retry can fix. **The engine must treat "process exited before the readiness line" as a distinct failure state**, not as a clean exit. The readiness signal is the `Done (` line, which appeared **1.3 s** after launch.
2. **A forced kill saves nothing, and there are two JVMs.** Killing the process produced **no shutdown or save message** in the log — it jumps straight from the periodic auto-save to the exit. And **two** `java.exe` processes had `server.jar` in their command line, of which only one owned the listening socket. Therefore: **(a)** the engine must hold a **writable stdin pipe** for the server's whole lifetime and write `stop` to it — on Windows there is no other graceful channel, no service control, and no signal; and **(b)** the engine must track the **process tree**, or stopping the listening PID orphans the other JVM. Both are hard requirements for `SRV-007`, and neither is in the task text today.

**One new behavioural fact worth having.** After 60 s with no players the server logged **"Server empty for 60 seconds, pausing"** and paused. This is good for idle CPU, and it is a trap for our UI: **a tunnel can be active while the server is paused.** The players-seen indicator must not treat a live tunnel as evidence of activity (`FR-012`, `PERF-001`).

**What this does not settle.** Nothing here was run on a **clean standard-user VM**, so `SRV-001`'s central promise — *no manual steps, no admin prompt, on a machine that has nothing installed* — is **still unverified**. This machine already had Java 25, which is precisely the condition the real product must not assume. `E-19`'s footprint half is answered for a developer machine only. **The Java runtime part of that gap is closed in RQ-13.2c's successor, §5 RQ-13.2d, by running the server on a private per-user JRE instead of the machine's own.**

#### RQ-13.2d The JRE question is answered: a per-user runtime needs no elevation, and the `SRV-007` stdin requirement is now proven (2026-10-17)

RQ-13.2c left one question that mattered more than the rest: a typical host has no Java 25, `SRV-010` forbids a system-wide install, and the product cannot assume anything is present. **That is now answered by measurement**, and the answer removes the blocker rather than relocating it.

**Step 1 — is there a Mojang-published runtime? Not established. [X]** The version JSON names a runtime component (`java-runtime-epsilon`), and Mojang documents runtime manifests under `launchermeta.mojang.com/v1/products/java-runtime/<component>/<os>-<arch>/<major>.json`. **36 probes** across four component names, three Windows architectures and three major versions **all returned HTTP 404.** I stopped there rather than trying more URL shapes, because guessing further is exactly the invention `AGENTS.md` forbids. **[X] unestablished** — not "does not exist", but "not found". This matters: Mojang's own runtime would have been the most defensible option, same vendor as the artifact and version-matched, and it could not be shown to be available.

**Step 2 — a third-party runtime exists with a verifiable checksum. [V]** Adoptium's documented API v3 returned exactly one asset for Java 25 / Windows x64 / JRE: **`OpenJDK25U-jre_x64_windows_hotspot_25.0.4.1_1.zip`, 58,475,080 bytes, SHA-256 published**. Downloaded and verified: byte count matched and the computed SHA-256 was an **exact match**.

**Step 3 — it needs no elevation, because it is a ZIP. [V]** A ZIP has no installer: no UAC prompt, no service, no registry write, no `PATH` change. Extracted to a temporary directory — 382 entries, `java.exe` at `…/bin/java.exe`, and confirmed **not present on `PATH`**, so a genuinely private copy rather than the machine's existing Java.

**Step 4 — the private copy runs standalone. [V]** Launched with a deliberately stripped environment, `PATH` set to `C:\Windows\System32` only and **`JAVA_HOME` unset** so no system Java was reachable, it reported `openjdk version "25.0.4.1" 2026-08-18 LTS`, exit 0.

**Step 5 — and it runs the real server. [V]** Under that same stripped environment `server.jar` reached **"Done (1.651s)!"** and `127.0.0.1:25565` was reachable. **The "host has no Java" scenario is solved.** Not elevated throughout; 0 services matching `jdk|java|temurin|minecraft`.

**Step 6 — the `SRV-007` stdin requirement is now proven rather than inferred. [V]** RQ-13.2c inferred from a forced kill that a writable stdin pipe was required. That inference is now an A/B, run in a single process:

| Action | Result |
|---|---|
| Write `stop\n` to the server's stdin | **"Stopping the server" → "Saving players" → "Saving worlds" → "Saving chunks"** for all three dimensions, **exit 0 in 0.5 s** |
| Force-kill the process (earlier run) | **No shutdown or save message at all** |

So the requirement in `SRV-007` — hold a writable stdin pipe for the process lifetime — is the only difference between a saved world and a lost one on Windows. Not a stylistic preference.

**A methodological note, because my first attempt at that test was invalid.** The first graceful-stop test lost the stdin handle when the *parent* process exited, so `stop` could never be sent and the test proved nothing. The handle must be held for the child's whole lifetime — which is exactly the property `SRV-007` requires of the engine, discovered by being unable to test it. **A test that cannot fail is not a test; this one initially could not.**

**Two further findings.**

1. **`--initSettings` is not optional if we want a clean log.** Starting with no `server.properties` logs `[ServerMain/ERROR]: Failed to load properties from file: server.properties` with a `NoSuchFileException` stack trace, then falls back to defaults and starts normally. Functionally fine, but a host reading their log sees a red ERROR on a successful start. Provisioning must run `--initSettings` first, which is what RQ-13.2c showed it does.
2. **Footprint is ~307 MB per provisioned Minecraft Java server, not the ~126 MB measured earlier.** Measured: private JRE **179.1 MB / 320 files** (from a 58.5 MB ZIP) plus server **127.9 MB / 76 files** = **307 MB, 400 files**. That is the real number for `SRV-001`'s disk budget and for the installer-size decision.

**What remains open, and the recommendation it supports.**

- **[X] Redistributability is not verified.** The Adoptium assets endpoint returned `licenses: null`, so **no licence claim is made here.** Bundling a runtime would require reading the project's licence first and adding a `docs/dependency-register.md` entry per `AGENTS.md` §5. **Fetching at provision time from the vendor's own API redistributes nothing**, which sidesteps the question for the first cut — and is why that option is preferable rather than merely simpler.
- **Still `[X]`:** whether a *typical* host can reach `api.adoptium.net`, and whether the GitHub release CDN its asset link points to is reachable on the networks we care about. That is `E-14`'s restrictive-network question applied to a second vendor, and it is a real risk.
- **The recommendation — DECIDED 2026-10-17 by the product owner as "verified-then-host":** fetch the runtime at provision time from Adoptium's documented API, verify the published SHA-256 before use, extract per-user, and **do not bundle it**. Fall back to a validated host JRE, then to a clear, specific refusal. Fetching rather than bundling keeps the first cut free of redistribution questions and keeps our own installer small. The two-step ordering was chosen over refuse-only; the trade is recorded honestly in RQ-13.2e.

#### RQ-13.2e What "a suitable host JRE" actually means on Windows, and the trap that decides it (2026-10-17)

The recommendation above needs a refusal path, and a refusal path needs a definition of "suitable". That definition was nowhere in the documents, and it is not the obvious one. Measured on this machine, which turns out to be an awkward case.

**The trap, demonstrated rather than described. [V]** This machine has **two Java generations installed**:

| Discovery route | What it finds here |
|---|---|
| `java` on `PATH` | `C:\Program Files\Common Files\Oracle\Java\javapath\java.exe` — an Oracle **forwarder shim**, not a real JRE. Reports 25.0.1 |
| Registry `HKLM\SOFTWARE\JavaSoft\JDK` | `25.0.1` |
| Registry `HKLM\SOFTWARE\JavaSoft\Java Runtime Environment` | **`1.8`** and `1.8.0_481` — a **Java 8** runtime, also installed |
| `C:\Program Files\Java` | `jdk-25`, `jre1.8.0_481`, `latest` |
| `JAVA_HOME` | **unset** |
| Registry `HKLM\SOFTWARE\Eclipse Adoptium\JDK` | **absent** — so a Temurin-specific key is not a reliable discovery route |

Running the server on the Java 8 runtime fails immediately and unambiguously:

```
Exception in thread "main" java.lang.UnsupportedClassVersionError:
  net/minecraft/bundler/Main has been compiled by a more recent version of the
  Java Runtime (class file version 69.0), this version of the Java Runtime
  only recognizes class file versions up to 52.0
```

Class file version **69.0 is Java 25**; **52.0 is Java 8**. The failure happens on the *first class load*, before any server logic runs.

**Three design consequences. [V] for the measurements, [A] for the conclusions.**

1. **A Java 8 runtime on the host is a realistic, silent hazard, not a hypothetical.** Naive discovery — "read the registry", "run `java -version`", "just use whatever is on `PATH`" — can all land on Java 8 here and produce a crash the host cannot interpret. Discovery must **validate by running `-version` and parsing the major number**, never by trusting a path, a registry key, or a `PATH` entry. The `javapath` shim is why `PATH` is a poor source of truth: it resolves successfully while pointing somewhere else entirely.
2. **The required version is data, not a constant.** It comes from `javaVersion.majorVersion` in the version JSON (RQ-13.2b), so it is **read per game version and never hard-coded**. A host on Java 21 who downloads a server needing 25 must be told 25, not "a newer Java".
3. **The refusal can be specific, because the failure is specific.** Because the mismatch surfaces on the first class load, the app can detect it *before* launching and refuse with a message naming the required major version and what it actually found — rather than surfacing a Java stack trace. That is the difference between a support ticket and a self-service fix.

**The resulting runtime-selection contract, recorded as `SRV-018`.** Order of preference: **a JRE the app itself fetched and verified** (RQ-13.2d) → a **validated** host JRE at or above the required major version → otherwise **refuse, naming both versions**. Registry reads are read-only `HKLM` queries and need no elevation, which is what makes the middle step possible at all under `SRV-010`.

**The ordering was decided by the product owner on 2026-10-17: verified-then-host.** It is therefore policy, not `[A]` reasoning. The owner was told plainly that it trades a verified artifact for a convenient one — RQ-13.2d checksum-verified the runtime we fetched, and a host-installed JRE carries no such guarantee — and chose the two-step order, declining the stricter refuse-only alternative.

**What that acceptance means, stated honestly rather than buried.**

1. **The second step is a compatibility check, not an integrity check.** Running `-version` and parsing the major number proves the runtime is *new enough*. It proves nothing about whether the binary is intact or untampered. The first step of `SRV-018` is the only one with an integrity guarantee.
2. **So the residual risk is a compromised host JRE**, which the app would use in preference to failing. On the threat model this project has adopted — supply-chain risk on the host's machine, `SRV-002` — that is a real, accepted exposure, and it is the reason the first step exists at all: on a host with no suitable Java we fetch and verify rather than trust.
3. **This is an accepted risk, on the same footing as ADR-019's separation-of-duties gap.** Both are recorded as accepted rather than mitigated, because the alternative was available and was declined.
4. **It is revisitable cheaply**, because nothing about it is structural: the order is a preference list, not a dependency. If a future version decides the exposure is not worth it, the change is to delete step two — no schema, contract, or architecture change follows.

#### RQ-13.2f The clean-VM test cannot run here, and the reason is a hard prerequisite worth recording (2026-10-17)

`SRV-001` and E-19's remaining gate is a clean standard-user VM. Before asking the owner for "a machine", I checked whether this one could be made to serve. **It cannot, and the reason is not elevation.**

| Attempt | Result |
|---|---|
| **Windows Sandbox** | **Unavailable, and unfixable here.** This machine is **Windows 10 Home Single Language** (build 26300). Sandbox requires **Pro, Enterprise, or Education** — it is not a feature Home lacks *enabled*, it is a feature Home does not have. `WindowsSandbox.exe` and `WindowsSandboxClient.exe` are both absent |
| Hyper-V / Virtual Machine Platform / WSL | `Get-WindowsOptionalFeature -Online` **requires elevation** for every feature queried, so their state cannot even be read unelevated |
| Elevation on this session | Not available, and `AGENTS.md` §9.5 forbids taking it without an explicit instruction |

**So the owner's "a machine" has a prerequisite that would otherwise cost a wasted round-trip: it must be Windows Pro, Enterprise, or Education.** A Home machine cannot run this verification by any means available to a non-elevated session, which is worth stating plainly rather than discovering later.

**What was done instead, and it is real evidence about the code even though it is not evidence about a clean VM.** `scripts/verify_clean_vm.py` had never exercised the `SRV-018` **fetch** path, because this machine always satisfies the host-JRE step first — so the branch a genuinely clean VM *must* take was the one path never run. A `--simulate-no-host-java` flag was added and both branches were then verified end to end:

| Branch | Result |
|---|---|
| Host JRE (step 2) | **11/11 checks**, exit 0 |
| Fetched, checksum-verified JRE (step 1) | **12/12 checks**, exit 0 — SHA-256 verified, extracted per-user, `java -version` re-read as 25, server reached readiness in 7.0 s, `127.0.0.1` and `::1` probed, graceful stop with save messages, **398 files / 321.9 MB**, no service, no scheduled task |

**An environment-variable approach was tried first and does not work**, recorded because the failure is instructive: PowerShell's `$env:ProgramFiles = <empty>` does not displace the inherited `PROGRAMFILES` entry that Python's `os.environ` reads, so Java discovery kept finding the real runtimes. A flag states what it does; an env var that silently does not is worse than either.

**What this does and does not establish.** It establishes that **both `SRV-018` branches work**, that the integrity chain is correct, and that the script's own traps are handled. It does **not** establish `SRV-001`, because a simulated clean host still has this machine's Windows, firewall, Defender, network filters and disk behind it. The script prints that caveat itself whenever the flag is used, so a simulation cannot be mistaken for the real thing.
- [V] **RCON and query exist**: `enable-rcon`, `rcon.password`, `rcon.port`, `enable-query`, `query.port`, `broadcast-rcon-to-ops` (present since 1.0.0–1.0.15).
- [V] **New since 1.21.9: the Minecraft Server Management Protocol (MSMP)**, a server management API. From 25w35a MSMP clients must authenticate to access it; from 25w37a MSMP TLS is on by default. **Read in full — see RQ-13.4, which changes ADR-014.**
- [V] A GUI appears unless `--nogui`; headless operation is supported. Size ≈ 54 MiB jar, ≈ 114 MiB with generated data.

#### RQ-13.3 Bedrock Edition (BDS)

- [V] Distributed by Mojang Studios as a ZIP, free of charge, EULA-licensed (text not read). Windows 10 version 1703 or later, or Windows Server 2016 or later; the wiki states Windows 10 build 10.0.15063.
- [X] Official download page `minecraft.net/en-us/download/server/bedrock` — **not reachable** from this environment, so the exact download URL, its stability, and whether any published checksum exists are **unverified**. Without a checksum, `SRV-002` cannot be satisfied for Bedrock, and by `SRV-003` Bedrock would **not be provisionable**. This is currently the single largest risk to the feature.
- [V] **BDS has no RCON and no query.** The official Microsoft properties reference lists the full key set (~45 keys) and contains neither `enable-rcon`/`rcon.password`/`rcon.port` nor `enable-query`/`query.port`. The wiki limitation list independently states BDS "doesn't support remote console (RCON)". Two independent sources agree.
- [V] Control is via the **console on stdin/stdout**: `op`, `deop`, `gamerule`, `stop`. Supervision needs no remote-management protocol at all.
- [V] `enable-lan-visibility` defaults to **`true`** and "will cause the server to bind to the default ports (19132 and 19133) **even when** `server-port` and `server-portv6` have non-default values". This **upgrades the long-standing `[R]` claim in §2 to [V] as documented behavior**; actual runtime binding is still [X] and matters for GAME-003's non-default-port requirement.
- [V] `server-port` default 19132 (IPv4), `server-portv6` default 19133 (IPv6), both 1–65535. Confirms the §2 `[R]` row.
- [V] `online-mode` defaults to `true`; "Clients connecting to remote (non-LAN) servers will always require Xbox Live authentication regardless of this setting", and Microsoft "highly recommends" enabling it for internet-facing servers. Since a tunnel makes the server internet-reachable, **keep this on**; `allow-list` defaults to `false`.
- [V] **Windows Defender raises a firewall permission prompt when BDS first starts.** Creating an inbound rule requires elevation. This appears to conflict with `SRV-010` / SEC-012 — **but the tunnel needs no inbound port at all**, so the correct behaviour is that the app adds no firewall rule and the host declines. [X] Unverified: whether declining breaks the server, and whether the prompt recurs on every start. **E-16 must answer this.**
- [V] **Loopback restriction:** by default a Bedrock *client on the same machine* cannot connect to BDS; Microsoft documents a `CheckNetIsolation.exe LoopbackExempt` command to lift it. [A] This should not affect a normal Win32 process such as our engine, but **if it does, it breaks `FR-011`'s local probe for Bedrock and must be detected and explained**. [X] E-16 must test an ordinary process reaching `127.0.0.1:19132`.
- [V] **`/reload` is unsupported** — "the server needs to be restarted every time the server settings are readjusted". Any generated-configuration change therefore requires a **stop → write → start** sequence; there is no live-apply path.
- [V] **Console and mobile players on Xbox, PlayStation and Nintendo Switch "can only connect through a custom DNS or a LAN connection".** This **answers AS-2 negatively for consoles**: a Bedrock console player cannot connect to a relay-allocated IP:port. Only Windows/Android/iOS-class clients can. GAME-003's scope statement must be corrected.
- [V] No SRV record support (bears on `architecture.md` §20 item 4); no MOTD setting; no initial/maximum memory settings (unlike Java's `-Xmx`). Size ≈ 83.8 MB Windows ZIP.

#### RQ-13.4 The Minecraft Server Management Protocol (Java edition)

Read in full 2026-10-17 (minecraft.wiki, "Minecraft Server Management Protocol"). [V] for documented behaviour. **This is a better answer than RCON for almost every purpose, and it partially resolves ADR-014.**

What it is: **JSON-RPC 2.0 over WebSocket**, namespaced under `minecraft`, added in 1.21.9 and now at protocol version **3.1.0** (26.3).

Access control and defaults — all important:

| `server.properties` | Default | Note |
|---|---|---|
| `management-server-enabled` | `false` | **Disabled by default.** Good. |
| `management-server-host` | `localhost` | **Localhost by default.** Good — keeps it off the network. |
| `management-server-port` | `0` | **Random port on every startup.** A supervisor must set a static port. |
| `management-server-secret` | generated and **written to `server.properties`** if empty | **Measured 2026-10-17: the server does generate one on first run, and it is 43 characters.** ⚠️ The wiki's "exactly 40 alphanumeric characters", recorded here before, is **wrong**. Presented as a bearer token or via `Sec-WebSocket-Protocol: minecraft-v1,<secret>`. Unauthorized requests get 401. **The app neither creates nor holds this value** — see ADR-015 |
| `management-server-tls-enabled` | `true` | **But requires a keystore** (`management-server-tls-keystore`, PKCS12). |
| `management-server-tls-keystore-password` | none | **Can come from the environment variable `MINECRAFT_MANAGEMENT_TLS_KEYSTORE_PASSWORD`, then a `-D` JVM argument, then the property.** |
| `management-server-allowed-origins` | empty | If left empty, **no client can connect**. |

Why it beats RCON:

- TLS on by default, with an authenticated secret, versus RCON's plaintext TCP and a password in the same properties file.
- Rich methods: players list and kick, allowlist, bans, IP bans, operators, `server/status`, `server/save`, `server/stop`, `server/system_message`, ~22 server settings, and gamerules.
- **Notifications**, including `minecraft:notification/players/joined` and `/left`, `server/started`, `server/stopping`, `server/saving`, `server/saved`, `server/activity`, `gamerules/updated`, and `world/upgrade_started` / `upgrade_progress` / `upgrade_finished` / `upgrade_failed`.
- `rpc.discover` returns the live API schema, so a client can negotiate rather than hard-code.

Three consequences for our own design:

1. **ADR-014 has a much better answer than "write a password to a file."** Recommendation [A]: **never enable RCON**; the app generates no RCON credential at all. If the host wants remote-style management, they enable MSMP, and **the server generates its own secret into its own properties file** — the app does not create or hold it. The one genuinely improvable credential is the TLS keystore password, which can be passed by environment variable instead of being written to disk. Both paths must still be covered by SEC-002's exception wording, so ADR-014 narrows rather than closes.
2. **The honesty model can be improved, for Java only.** `minecraft:notification/players/joined` and `/left` mean the *game server itself* can report who joined — not inferred traffic. `architecture.md` §12 and `user-flows.md` §0 currently state that true in-game presence is unmeasurable. That remains true in general and by default, but for a managed Java server the game can be asked directly. Recorded as **SRV-011 (P2)** rather than folded into FR-012, because it changes what the product may claim and must be reviewed.
3. **Configuration hazards to encode in T-035:** MSMP is off by default and `allowed-origins` empty means unusable; the port is random unless set static; the keystore is required for the secure default; and since 26.2 the management server starts *before* the game server, so `rpc.discover` and server status work during world load — useful for startup health detection. `world/upgrade_*` notifications make the update path observable (SRV-008, SRV-009).

**See also RQ-13.4, which changes this recommendation and narrows ADR-014.**

Not resolved by MSMP: it is **Java edition only**. Bedrock still has console-only control via stdin, which is sufficient for start, stop and `op` but gives no player join events.

#### RQ-13.5 What this changes

1. **`SRV-004` was written as if RCON and query applied to both games. They are Java-only.** For Bedrock the requirement is vacuous, and the UI must not offer an RCON toggle for it.
2. **ADR-014 narrows further, with a better answer available (RQ-13.4):** do not generate an RCON credential at all; if management is wanted, use MSMP, whose secret the server generates for itself. What remains is SEC-002's wording for a server-owned secret that lives in the server's own properties file, plus the keystore password, which can be supplied by environment variable instead.
3. **AS-2 is answered: no, console/mobile Bedrock players cannot use a relay address.** This corrects `GAME-003` and the §2 feasibility matrix.
4. **The honesty model can improve for managed Java servers only** — the game can be asked directly who joined. Recorded as SRV-011 (P2), not folded into FR-012.
5. Three new unknowns, all load-bearing: the Bedrock checksum/integrity question, the firewall prompt under a no-elevation promise, and the loopback question for the local probe.
6. **The feature is still not justified.** The legal question (AS-11 / E-19) and the Bedrock integrity question are both open, and either can remove a game from the first cut.

### ADR-004 options (still open): can the relay see game traffic?

The answer depends on a product question I should have asked earlier: **do guests need to install the client?** (ADR-007, open)

| Guest model | What it means | Relay visibility |
|---|---|---|
| G1: guests install the client | Host and guest both run the app; a host-to-guest key exchange is possible | Relay can forward ciphertext only, if an end-to-end layer is added |
| G2: guests connect to a relay address with the stock game | Simplest guest UX; the relay must terminate the tunnel and talk to the game directly | Relay necessarily sees game traffic; only the client-to-relay hop is encrypted |

Trade-offs [A]: G2 matches "share a connection address with friends" and works for console and mobile Bedrock players who cannot install the app, but it makes the relay a trusted party and a bigger abuse and privacy target. G1 allows stronger privacy but excludes players who cannot run the client. A hybrid (G2 default, optional G1) is possible but adds scope. This must be decided by the product owner before the security architecture is finalized.

## 6. Security note carried from research

Tailscale-style relays forward *already encrypted end-to-end* traffic, so the relay cannot read payloads [R]. In this product the game protocol is generally **not** end-to-end encrypted, and the relay terminates the tunnel transport. [A] Documentation must say plainly: transport encryption protects client-to-relay segments, and the relay operator can in principle see game traffic unless a separate end-to-end layer is added. This is an open design decision (ADR-004 below).

## 7. Architecture Decision Records (drafts)

| ID | Decision | Status | Date | Confidence |
|---|---|---|---|---|
| ADR-001 | UDP transport: QUIC DATAGRAM for UDP, QUIC streams for TCP | Proposed | 2026-10-16 | Low–medium |
| ADR-002 | Relay-first MVP; direct P2P deferred pending E-7 | Proposed | 2026-10-16 | Medium |
| ADR-003 | Relay never connects to client-supplied destinations; destination fixed by host-side mapping validated against game profile | Proposed | 2026-10-16 | High (design principle) |
| ADR-004 | Whether to add an end-to-end encryption layer between host and guest (depends on ADR-007) | Open | 2026-10-16 | n/a |
| ADR-005 | Tauri 2 shell + Rust engine, engine as current-user in-process or child process, no Windows service | Proposed | 2026-10-16 | Low–medium |
| ADR-006 | Per-user NSIS installer + signed Tauri updater; MSIX deferred | Proposed | 2026-10-16 | Low–medium |
| ADR-007 | Guest model: **G2 — guests connect to the relay address with the stock client; no guest client, no guest account.** Decided by the product owner 2026-10-17, **which satisfies `SRV-014` by construction rather than by weakening it** | **Decided 2026-10-17** | 2026-10-16, decided 2026-10-17 | **[V]** that this is the owner's decision. **[A]** on the consequence analysis. Guest admission is now wholly IP-based, which makes E-13 load-bearing and still `[X]` |
| ADR-008 | Rust control plane (`axum`) + PostgreSQL (`sqlx`); relay state in memory | **Accepted 2026-10-17** by the product owner | 2026-10-16, recorded 2026-10-17 | Medium |
| ADR-009 | System-browser OAuth (PKCE) + OS credential store; identity provider open | Proposed | 2026-10-16 | Medium (flow), none (provider) |
| ADR-010 | `tracing` + optional OTLP; no default client telemetry | Proposed | 2026-10-16 | Medium |
| ADR-011 | Compatibility test levels L0–L2; "supported" requires L2 | Proposed | 2026-10-16 | Medium |
| ADR-012 | Host-local game server provisioning runs as an engine-supervised child process on the host's PC; never operator-hosted | Proposed | 2026-10-17 | Low (no evidence; E-15–E-19) |
| ADR-013 | First provisioning cut is **Minecraft Java Edition only**. Bedrock deferred pending a published integrity mechanism; Terraria, L4D and L4D2 stay manual | **Decided 2026-10-17** | 2026-10-17 | Medium. Based on documented behaviour, not on a run |
| ADR-014 | Treatment of a generated game-server credential that the server must read from its configuration file | Open, **narrowed twice** — now Java-only (Bedrock has no RCON), and RQ-13.4 offers a better answer | 2026-10-17 | n/a |
| ADR-015 | Prefer MSMP over RCON for Java server management; never enable RCON; let the server generate its own MSMP secret; pass the TLS keystore password by environment variable | Proposed, **now exercised 2026-10-17** | 2026-10-17 | **Raised to high** — the behaviour was observed on a running server, not just read. See §5 RQ-13.2c |
| ADR-016 | Cited files must exist; expected absences must be declared explicitly | **Accepted 2026-10-17** | 2026-10-17 | High (a demonstrated defect, not a judgement) |
| ADR-017 | `sqlx migrate` owns `migrations/`; no separate migration library | **Accepted 2026-10-17** | 2026-10-17 | Medium-high; tool not yet executed, T-033 must verify |
| ADR-018 | Admin access tokens will **not** be sender-constrained at launch; PKCE challenge method is fixed to `S256` with no `plain` fallback | **Accepted 2026-10-17** by the product owner | 2026-10-17 | High for the S256 half (RFC 9700 is normative and read directly); the deferral half is a recorded sequencing judgement |
| ADR-019 | The project owner is the sole administrator; admin is a **separate identity** (not a role on `users`); WebAuthn required, no TOTP fallback; short sessions plus step-up before destructive actions; recovery codes. **Separation of duties is absent and recorded as an accepted risk** | **Decided 2026-10-17** by the product owner | 2026-10-17 | [V] that this is the decision; [A] on all design reasoning, none of it tested |

**ADR-016 was missing from this table on 2026-10-17** even though its full record existed 30 lines below it, and ADR-017 was present. Found while adding ADR-018. That is the same defect class ADR-016 was written about: a claim that exists in the document but is not represented where a reader would look. The summary table is now checked by `scripts/audit_traceability.py`, which was extended to require that every `### ADR-nnn` heading in §7 has a matching row above it — the same check that would have caught this.

Each ADR still needs full context, options, rationale, and consequences sections. Not yet written, except ADR-016 and ADR-017, which are complete.

### ADR-016 — Cited files must exist, and expected absences must be declared

**Status:** Accepted, applied immediately | 2026-10-17 | High (a demonstrated defect, not a judgement)

**Context.** On 2026-10-17 it was discovered that `docs/contracts/` and `contracts/examples.json` did not exist, while `contracts.md`, `api.md`, `data-model.md` §7, `tasks.md` T-031, and `review-report.md` §2 all described or validated them. The claim had survived an entire review cycle. Two distinct failures were involved: the claim was false, and **every existing mechanical check passed anyway.**

**Decision.**
1. `scripts/audit_traceability.py` now verifies that every cited repository path exists. This is a **hard error**, because a document set that points at files which were never written is broken regardless of internal consistency.
2. Files a task will legitimately *produce* are declared in an `EXPECTED_ABSENT` set and reported as **warnings**, not errors — a planning-phase repository is allowed to name artifacts that do not exist yet. Removing an entry from that set is a deliberate act that should be reviewed, and the set carries a comment naming the task that owns each file.
3. The defect class is named so it can be recognised: **an internally self-consistent document set can still be wrong.** Identifier and count checks cannot catch a file that was never authored, because the identifier is spelled consistently everywhere.

**Consequences.** T-031's "Done when" is now known to be unachievable as written and was corrected. `contracts.md` records itself as the only wire-format source of truth that exists. The audit's self-test gained a sixth planted defect (a cited path that does not exist) so the new check cannot silently stop working.

### ADR-008 (expanded) — Rust control plane with PostgreSQL

**Status: Accepted, 2026-10-17, by the product owner.** Previously this ADR was a single table row at *low* confidence, recorded with no context, options, rationale, or consequences. It is now a full record, which is what the note under the ADR table requires of every entry.

**Context.** The control plane is the authenticated HTTPS service owning identity, authorization, tunnel and invitation lifecycle, relay allocation, quotas, and audit. Three components interoperate over one wire protocol: the control plane, the relay, and the in-process client engine. The relay and engine are already settled as Rust (`architecture.md` §2, ADR-001, ADR-005). Choosing the control plane in another language therefore has a cost — duplicating protocol types and token validation in a second language, or introducing a code-generation step — which is a build-time and review burden, not just a preference.

**Options considered.**

| Option | Case for | Case against |
|---|---|---|
| **A. Rust (`axum`) + PostgreSQL (`sqlx`)** | Shares the protocol crate with the engine and relay; one language and one set of types across all three components; no code generation | Adds a substantial dependency tree to a component that currently has none; `sqlx` pulls 145 transitive crates and declares MSRV 1.94, forcing the workspace up from 1.77 |
| B. TypeScript on Node | Faster iteration if the team is stronger there; a shared schema package could carry the contract | Protocol types exist in two languages with nothing enforcing agreement; the relay stays Rust regardless, so a toolchain split persists anyway |
| C. C# on ASP.NET | Mature, good tooling | Same duplication problem, plus a third runtime |
| D. Go | Strong concurrency story for a network service | Same duplication problem; weakest case for protocol sharing |

**Evidence gathered 2026-10-17** (§5 RQ-10): `axum` 0.8.9 MIT, repo pushed 2026-10-15; `tokio` 1.53.3 MIT, pushed 2026-10-16; `sqlx` 0.9.0 Apache-2.0, pushed 2026-10-16. None archived. All licences are already permitted by `deny.toml`. `sqlx` compiled in this environment on the GNU toolchain and `cargo deny` passed on its real 145-crate graph. **This table is not a head-to-head benchmark:** options B, C and D were not measured or costed at all.

**Decision.** Option A. Rust control plane using `axum`, PostgreSQL via `sqlx`, durable state in PostgreSQL, relay session state in memory only.

**Rationale.** Protocol sharing is the deciding factor. The control plane issues the signed allocation tokens the relay verifies offline (`architecture.md` §6) and the engine presents them, so token parsing, claim validation, and message types must not be able to drift between three components. One shared crate removes that class of bug and removes any code-generation step from the build.

**Consequences, accepted deliberately.**

1. **The workspace MSRV rises from 1.77 to 1.94**, because `sqlx` declares `rust-version = "1.94.0"`. Recorded rather than discovered later.
2. **The control plane gains a substantial dependency tree.** Every entry needs a dependency-register row in the same change (`AGENTS.md` §5.1). T-030, which builds that register, is unblocked by this decision.
3. **The migration-tool choice becomes decidable** (RQ-14): `sqlx migrate` is no longer "a driver plus a separate tool" but one dependency. **The tool is still not chosen** — RQ-14 keeps `refinery` and hand-rolled SQL open, and choosing remains T-003's first deliverable.
4. **If the team's TypeScript experience later turns out to dominate Rust experience, this should be revisited as a new ADR, not a silent migration.** The deciding factor would need re-evaluating against a concrete shared-schema-package design.
5. **This says nothing about `quinn`** (ADR-001, RQ-6), the data plane's transport, which remains unproven until E-6.

**Not settled here.** Identity provider (ADR-009) and the admin authentication model (SEC-018) are untouched; SEC-018 is still undesigned. **Evidence for SEC-018 was gathered on 2026-10-17 — see RQ-15.**

### RQ-15: What does "stronger authentication" mean for the admin API?

Added 2026-10-17. `SEC-018` (P0) requires that admin operations "require separate, stronger authentication", and `contracts.md` §9 says this section "must be completed before the security review" — but no definition of "stronger" existed anywhere. That is not implementable as written. This section gathers primary-source evidence and lays out options; **it does not decide**, because the decision depends on the identity provider, which is still open (ADR-009).

**Sources read 2026-10-17** (all `[V]`, read directly from the RFC text, not from summaries — a deliberate correction of the practice recorded in RQ-9, where RFCs were confirmed only via secondary sources):

| Source | Title | Relevance |
|---|---|---|
| **RFC 9700** (BCP 240, Jan 2025) | Best Current Practice for OAuth 2.0 Security | The normative reference for what "secure OAuth" now means |
| RFC 8705 | OAuth 2.0 Mutual-TLS Client Authentication and Certificate-Bound Access Tokens | Sender-constraining by client certificate |
| RFC 9449 | OAuth 2.0 Demonstrating Proof of Possession (DPoP) | Sender-constraining by proof-of-possession |
| RFC 8252 (re-read) | OAuth 2.0 for Native Apps | External-user-agent requirement |

**What RFC 9700 actually requires** — quoted from the text, because this is the part that changes our contract:

- **"Public clients MUST use PKCE"**, and PKCE for confidential clients is RECOMMENDED, specifically "to prevent authorization code injection attacks". Our `contracts.md` §5.1 already requires PKCE, so current design conforms. [V]
- **"Currently, S256 is the only such method."** Any `plain` PKCE method in our contract would be non-conforming. `contracts.md` §5.1 says "verifier must match" but **never names the method** — an unspecified-method gap. [V]
- **"Refresh tokens for public clients MUST be sender-constrained or use refresh token rotation"** (§2.2.2). We chose rotation with family revocation on reuse, which satisfies the second branch. Our §5.2 wording matches. [V]
- **Sender-constrained tokens** are the RFC's answer to stolen/leaked access tokens, delivered via RFC 8705 (mTLS, "certificate-bound", 30 mentions) or RFC 9449 (DPoP, whose `jti` claim and proof validation are normative). [V]
- Authorization code injection and mix-up attacks are treated as distinct threats (11 and 9 mentions respectively). RFC 9700 notes OpenID Connect clients **MAY** use `nonce` instead of PKCE — relevant only if an OIDC provider is chosen. [V]

**Note a gap in our own documents:** RFC 9700 requires PKCE for **public clients**, and a desktop app using a system browser is a public client. Our `SEC-001` and §5.1 treat PKCE as the whole story. They do not mention sender-constraining access tokens at all, for any client type. **This is an unresolved design question, not a violation** — sender-constraining an access token is `SHOULD`-level guidance for authorization/resource servers — but it should be decided deliberately rather than inherited by omission.

**Options for "separate, stronger" admin authentication.** None chosen.

| Option | Shape | Cost / risk |
|---|---|---|
| **A. Separate IdP tenant, WebAuthn-only** | Admin identities live in a different identity provider tenant from end users, and **only** phishing-resistant second factors are accepted for the admin role | Strongest separation. Requires an IdP that supports WebAuthn and per-app configuration; if the chosen IdP is a single consumer-facing tenant this collapses into option B. **Note: RFC 9700 does not use the term "phishing-resistant" (0 mentions), so this option rests on WebAuthn/W3C guidance, not on RFC 9700.** |
| **B. Separate role + mandatory second factor, same IdP** | Admin is a role on `users` (`data-model.md` §2 already raises this question) with a mandatory TOTP/WebAuthn second factor at authorization time | Cheapest. Weaker: a compromised host session can still attempt escalation, and "mandatory second factor" must be enforced by our own authorization server, which is trust we may not own if the IdP is external |
| **C. mTLS client certificates (RFC 8705)** | Admin tooling presents a client certificate; tokens are certificate-bound | Standardised and strong; `contracts.md` §7 already leaves the relay credential as "mTLS vs signed token: **open**", so this reuses an unresolved decision. Operational cost of issuing and rotating operator certificates is real |

**Questions this cannot answer without the product owner**, and which are recorded rather than guessed:

1. **Who are the administrators?** The project has no named operator team, no operator headcount, and no stated relationship between the operator and the end users they would suspend.
2. **Which identity provider?** ADR-009 is still open. Option A is impossible until this is answered.
3. **Is an operator a human at all?** Automated administration (for example, an autoscaling controller draining relays) would change the design completely — a certificate or workload identity, not a second factor.
4. **What does "separate" mean operationally** when a one-person project has one administrator who is also likely a developer with database access? Separation of duties is weakest here.

**Question 1 was answered by the product owner on 2026-10-17: the administrator is the project owner.** See ADR-019. That answer, plus the consequence recorded there, **decouples admin authentication from the identity provider**, which means question 2 is no longer a blocker for SEC-018 — it remains open for *users* under ADR-009.

**What is actually needed to unblock SEC-018**: recorded in ADR-019, which states the decidable part and names what remains open. **Confidence: the RFC evidence is [V]; the option assessment is [A] reasoning and was explicitly incomplete, and ADR-019 records which parts survived that scrutiny.**

### ADR-017 — `sqlx migrate` owns the migrations directory

**Status: Accepted, 2026-10-17.** Decided by the product owner on the recommendation that `sqlx migrate` be used.

**Context.** T-003 required a migration tool. `schema.md` fixes the workflow that the tool must implement: forward-only files, one migration step per deployment, never edit an applied migration, expand/contract column changes, and rollback by compensating forward migration rather than by history rewriting. The schema itself is now verified against PostgreSQL 16.10 and 17.6 (see `schema.md`), so the tool has a known-good target.

**Options.**

| Option | Case for | Case against |
|---|---|---|
| **`sqlx migrate`** | The driver is **already** `sqlx` (ADR-008), so this is one dependency rather than a driver plus a separate migration tool. Reversible `.up.sql`/`.down.sql` pairs, matching the rollback rule. Actively maintained, permissively licensed, MSRV already accepted | 145 transitive crates, already counted in ADR-008's cost. CLI not executable in the authoring environment |
| `refinery` | Lighter, single binary, embeds migrations | A **second** migration dependency alongside an `sqlx` driver; no prebuilt Windows binaries, so it must be compiled from source |
| Hand-rolled numbered `.sql` | Zero dependencies, full control, and `schema.md` already describes the workflow tool-neutrally | The bookkeeping — ordering, applied-tracking, checksums, and **enforcing that an applied migration is never edited** — becomes ours to get right. `data-model.md` §8 depends on that enforcement |

**Decision.** `sqlx migrate`, from the `sqlx` dependency already accepted in ADR-008.

**Rationale.** The deciding factor is that the driver decision has already been made. Choosing `sqlx migrate` costs **zero additional dependencies**, while `refinery` would mean two migration libraries in one service. The reversibility requirement in `schema.md` maps directly onto `sqlx`'s up/down pairs. Hand-rolling was rejected on the specific ground that `data-model.md` §8 forbids editing an applied migration — an invariant best enforced by a tool that tracks applied checksums rather than by convention.

**Consequences.**

1. **`schema-draft.sql` is retired in T-033**, replaced by `migrations/` files, exactly as `schema.md` rules 1 and 2 require. Until then the two must be kept in agreement.
2. **T-033 must verify rather than assume.** `sqlx-cli` could not be run here — `dlltool.exe` is missing from the GNU toolchain, the MSVC toolchain has no Windows SDK, and no prebuilt binary is published. T-033's first check must be that `sqlx migrate` parses statement boundaries correctly, because that is the exact defect class which silently produced a **zero-table** schema on the first manual apply (recorded in `schema.md`).
3. The migration tool ships with the dependency already counted in ADR-008; the register needs no new row beyond the existing `sqlx` entry.
4. **`invitations`, `guest_admissions`, and `relay_allocations` all change shape** as experiment E-13 resolves, so early migrations are expected to be followed by compensating forward migrations. That is the documented workflow, not a failure mode, but T-033 should not treat the initial schema as immutable.

### ADR-018 — Fix PKCE to `S256`; defer sender-constraining access tokens

**Status: Accepted, 2026-10-17.** Confirmed by the product owner. This is deliberately a *small* ADR: it records one change that is forced by a normative source, and one deferral, without pre-empting SEC-018.

**Context.** Reading RFC 9700 (BCP 240) directly on 2026-10-17 (RQ-15) surfaced two things our documents get wrong or leave silent.

1. **Our contract never names the PKCE challenge method.** `contracts.md` §5.1 requires that the "verifier must match" and nothing more. RFC 9700 §2.1.1 states: *"Currently, S256 is the only such method."* A contract that does not name the method cannot reject a `plain` challenge, and a `plain` challenge provides no protection against an attacker who can observe the authorization request.
2. **We never considered sender-constraining access tokens at all.** RFC 9700 §2.2 treats them as the answer to stolen and leaked access tokens, via RFC 8705 (certificate-bound) or RFC 9449 (DPoP). Our `SEC-001` treats PKCE as the whole story.

**Options for the first item** — there is effectively only one: require `S256` and reject `plain` outright, since RFC 9700 leaves no room for `plain` and it must be rejected rather than merely discouraged.

**Decision (item 1, recommended).** The PKCE challenge method is fixed to **`S256`**; a `plain` challenge is rejected with `validation_failed`. `contracts.md` §5.1 names it explicitly.

**Decision (item 2, recommended).** Sender-constraining is **deferred**, recorded as an open question rather than adopted by omission. Rationale: it is `SHOULD`-level, it requires choosing a mechanism (mTLS or DPoP), and option C in RQ-15 collides with the still-open relay-credential question in `contracts.md` §7. Adopting it before SEC-018 and ADR-009 are settled would mean picking a mechanism for an identity model that does not exist yet.

**Rationale.** Item 1 costs nothing and removes a real defect. Item 2 is recorded so that the omission is a decision with a reason rather than a gap nobody noticed — which is precisely the failure mode ADR-016 was written about, and the third time this document set has claimed verification it did not perform.

**Consequences.**

1. `contracts.md` §5.1, `api.md`, and `SEC-001`'s acceptance criteria must name `S256` and add a negative test: a `plain` challenge is rejected. Mapped to T-043/T-046. **Done 2026-10-17** — all four places updated.
2. **The open question is now written down** in `research.md` RQ-15 and in the §7 table, so it cannot be lost between sessions.
3. **This does not unblock SEC-018.** The admin authentication model still needs the two owner answers named in RQ-15: who the administrators are, and which identity provider. `contracts.md` §9 stays OPEN. **Update 2026-10-17: the "who" answer has since been given** — the project owner is the sole administrator (ADR-019) — **so only the identity-provider question remains, and ADR-019's separate identity store means it no longer blocks admin design.**
4. **If the deferral is ever revisited**, DPoP (RFC 9449) is the cheaper of the two mechanisms for a desktop client, because it needs no operator certificate issuance. That is a later decision, not this one.

**Confidence.** High for item 1 — RFC 9700 is normative and was read directly. Low for item 2 — it is a judgement about sequencing, not a technical finding.

### ADR-019 — The project owner is the administrator

**Status: Decided 2026-10-17 by the product owner**, who answered RQ-15 question 1 with "make me the admin". This closes the *who* question. It does **not** close every part of SEC-018, and the consequences below include a weakness that authentication cannot fix.

**Context.** `SEC-018` (P0) requires that admin operations "require separate, stronger authentication". RQ-15 recorded that nobody had defined "stronger", and that four questions blocked a design. The owner has now answered the most important one: **there is one administrator, and it is the project owner.**

**Decision.**

1. **One administrator: the project owner.** A single human identity.
2. **Admin is a separate identity, not a role on `users`.** `data-model.md` §2 raised this as open; it is now closed toward a separate store. An admin identity is never also a user identity, so an admin session cannot be used to hold tunnels and a user session carries no admin capability.
3. **Strongest available factor: WebAuthn (passkey) required.** No TOTP fallback for the admin role. Rationale below.
4. **Short admin sessions, and step-up re-authentication before destructive actions.** "Destructive" = terminate a tunnel, drain a relay, suspend a user, rotate signing keys.
5. **Recovery is designed, not assumed** — offline recovery codes, hashed at rest, plus a documented database-level recovery path. See the single-point-of-failure consequence.
6. **All admin actions are audited** as `actor_type = 'admin'`, append-only. `data-model.md` §4 already models this; it becomes a primary control rather than a secondary one.

**The reframe that drives the design — what admin authentication actually protects against here.** The obvious reading of "separate, stronger authentication" is separation of duties: two people, so neither can act alone. **That does not exist in this project and cannot be created by adding a second factor.** The owner is also the developer with database access. So the honest statement of what this control buys is narrower:

| It **does** protect against | It **does not** protect against |
|---|---|
| A stolen or escalated **user** session reaching admin routes | The owner themselves, who can act on the database directly |
| An attacker holding a valid user token escalating to admin | Anyone who has already obtained database access |
| **Accidental** destructive action by the owner, via step-up confirmation | A single compromised administrator account — there is nobody to notice |

Everything in the right-hand column is why this is recorded as an **accepted risk**, not a mitigated one.

**Consequences, stated plainly.**

1. **Separation of duties is absent and is not fixable at this scale.** Recorded as an accepted risk. The compensating controls are the audit trail (detective, after the fact) and step-up confirmation (accidental damage only). **Neither stops a determined owner.** A future version with a real operator team should revisit this ADR; the audit trail is the thing that makes revisiting possible.
2. **A single administrator is a single point of failure.** Losing the only admin credential makes the service unadministrable — with no second person to restore it. This is the direct cost of the decision and it is why recovery codes plus a documented database-level path are in the decision rather than left open. **[A] reasoning — the specific recovery mechanism is unverified and should be tested at T-049.**
3. **WebAuthn is required rather than optional because the blast radius is total.** One identity can suspend every user and rotate every signing key. The enrolment cost is one passkey for one person, which is the cheapest case for a strong factor; there is no fleet to roll it out to.
4. **The audit trail becomes a primary control, not a formality.** With one administrator, `audit_events` is the only record that distinguishes legitimate admin action from misuse. This raises the bar on INV-5: admin audit rows must never contain the recovery material or tokens, exactly as the existing invariant requires. **Note the tension with SEC-002:** recovery codes are a secret that must exist somewhere, so they are an explicit, narrow exception with a stated location, not a general licence to write secrets to disk.
5. **This decouples admin auth from the identity provider, so it no longer blocks on ADR-009.** ADR-009 remains open for *users*. A separate admin identity store is verifiable without choosing a user-facing IdP, which is why RQ-15 question 2 stopped being a blocker.
6. **The audit trail records `actor_type='admin'` already**; no schema change is required for this decision. A separate admin identity store does add an entity, which is a `data-model.md` and migration change for T-033 — flagged, not made here.

**Still open, and honestly so.** Whether WebAuthn can actually be enforced depends on the admin client and is **unverified** — it needs a spike, not a decision, and is added to T-049's scope. Whether the owner is ever also a user is a product question nobody has asked and this ADR does not answer it; the decision above forces "never", which should be confirmed rather than inherited.

**Confidence.** [V] that this is the owner's decision — it was stated directly. [A] on the design reasoning and on every "consequence" above; none of it has been tested.

### ADR-007 — G2: guests connect to the relay address, with no client and no account

**Status: Decided 2026-10-17 by the product owner.** This closes the guest model **and** resolves the `SRV-014` conflict that a G1 decision would have created. **No requirement had to be weakened.**

**The decision.** `[V]` — **G2**: a guest connects to the relay's public address with the stock game client. They do not install our software, and they do not hold an account on this operator's platform.

**Why this resolves the conflict instead of bending around it.** §5 RQ-13.1b (constraint C2) requires that server access not be "limited to or controlled, directly or indirectly, by a player owning or having access to out-of-game content, products, or services." A G1 design makes an account on our platform a precondition for reaching the game server, which is precisely what the clause appears to forbid. **G2 removes the precondition entirely**, so `SRV-014` holds on its own terms and its acceptance criterion — *"no guest-facing step requires an account on the operator's platform"* — is satisfied by construction rather than by reinterpretation.

**This was recorded as a live conflict first, and that record was kept deliberately.** An earlier draft of this ADR documented the G1 decision together with the `SRV-014` clash and three ways to resolve it. It was replaced when the owner changed the decision, but the sequence is worth preserving in substance: **two documents disagreed, the disagreement was written down where both `architecture.md` and `requirements.md` pointed, and it was escalated rather than silently reconciled.** That is the behaviour `AGENTS.md` §2 and §10 ask for, and it is recorded here so a later reader can see the conflict was surfaced rather than discovered later.

**Consequences.**

1. **`SRV-014` passes as written.** No amendment, no documented legal reinterpretation, no accepted-risk annotation. It is the only P0 requirement in this project that was satisfied by a design choice rather than by narrowing.
2. **`contracts.md` §6 and `api.md` row 18 stay as they are** — `POST /v1/invitations/claim`, Auth: **none**. Neither changes. That is the practical measure of how well the settled contracts anticipated this decision.
3. **Guest admission is entirely IP-based**, which is why E-13 (`T-015`) and `AS-12` matter so much: with no client and no account, the relay's only way to recognise a guest is the source address it observes. That is the experiment this decision makes load-bearing, and it is still `[X]`.
4. **We lose whatever G1 was chosen for.** If the motivation was discoverability, an in-game join method, guest-side telemetry, or moderation tooling, none of that comes free now. **Not recorded, because the owner did not state it** — and if it turns out to matter, the cheapest route back is the permissive reading of C2 argued in RQ-13.2e, not a re-litigation of this ADR.
5. **The remaining exposure is technical, not legal.** `SRV-014` is satisfied; E-13 is not. CGNAT, shared NAT, and mobile address changes (all recorded in RQ-13.1b and `AS-12`) now sit on the critical path to a working guest join, where before they sat behind a licensing question.

**Confidence.** `[V]` that this is the owner's decision — stated directly. `[A]` on the consequence analysis above, none of it tested.

## 8. Open experiments

| ID | Question | Blocks |
|---|---|---|
| E-1 | Does Terraria need UDP 7777? | Terraria profile |
| E-2 | Bedrock join through UDP relay; non-default and IPv6 ports | Bedrock profile, ADR-001 |
| E-3 | Source direct-connect through relay | L4D/L4D2 profiles |
| E-4 | L4D2 lobby join via relay | L4D2 scope |
| E-5 | L4D (1) protocol capture | L4D profile |
| E-6 | QUIC datagram relay under loss, jitter, MTU limits | ADR-001 |
| E-7 | Direct-path success rate on target networks | ADR-002 evolution |
| E-8 | Tauri shell + loopback engine spike: startup, idle memory, accessibility | ADR-005 |
| E-9 | Update, failed-update, rollback, tampered-artifact test on clean Windows VM | ADR-006 |
| E-10 | Bytes per player-minute for each game; cost model per provider | Hosting choice, quotas |
| E-11 | Headless or scriptable real-client join for each game | ADR-011, release gating |
| E-12 | Sign-in spike: system browser + loopback/device flow + credential store | ADR-009 |
| E-13 | Source-IP admission of guests at the relay: behavior with CGNAT, mobile, shared NAT | Guest access design (ADR-007) |
| E-14 | Fallback carrier for tunnel when UDP to relay is blocked | Restrictive-network support |
| E-15 | Minecraft Java server artifact: official channel, terms, unattended download, integrity check, standard-user install, required runtime. **All answered `[V]` 2026-10-17** (§5 RQ-13.2b, §5 RQ-13.2c, §5 RQ-13.2d): channel, terms, end-to-end SHA-1 integrity, unattended download, start-up, local probe, generated defaults, graceful stop, and **a per-user Java 25 runtime that needs no elevation and runs the server with no system Java reachable**. **Remaining `[X]`:** whether a typical host can reach the runtime vendor's API and its CDN, and the clean standard-user VM run | ADR-013, SRV-001, SRV-002 |
| E-16 | Minecraft Bedrock server artifact: same questions, plus the license-acceptance step and the settings that must be generated | ADR-013, SRV-001, SRV-002 |
| E-17 | Supervising a headless server process: start, graceful stop, bounded crash restart, log capture, exit-code handling | ADR-012, SRV-007 |
| E-18 | Update, rollback, world backup/restore, and per-server disk/CPU/memory footprint | SRV-008, SRV-009 |
| E-19 | Licensing and end-user terms for the host operating these servers, plus a clean standard-user VM footprint check. **Licensing: answered (§5 RQ-13.1b). Footprint: measured on a developer machine (§5 RQ-13.2c, §5 RQ-13.2d) — 307 MB and 400 files per provisioned Minecraft Java server. Still `[X]`: a genuinely clean standard-user VM** | SRV-010, AS-11 |

## 9. Sources (accessed 2026-10-16)

Primary or near-primary:
- Valve Developer Community, "Source Dedicated Server" — https://developer.valvesoftware.com/wiki/Source_Dedicated_Server — ports 27015/27020/27005/26900.
- Valve Developer Community, "Source RCON Protocol" — https://developer.valvesoftware.com/wiki/RCON — RCON on TCP 27015.
- Tailscale blog, "NAT traversal, and how we're improving it (pt. 1)" — https://tailscale.com/blog/nat-traversal-improvements-pt-1 — direct vs DERP relay, hard NAT cases.
- Terraria Wiki, "Server" — https://terraria.wiki.gg/wiki/Dedicated_Server — TCP 7777 default (community wiki, not the developer).

Secondary (reported only):
- Arch Wiki mirror, Minecraft Java server — https://wikipedia.jakami.de/content/archlinux_en_all_maxi_2026-04/Minecraft/Java_Edition_server
- GameServerKings, Bedrock server.properties — https://www.gameserverkings.com/knowledge-base/minecraft/bedrock-server-properties/
- Steam Community L4D2 threads — https://steamcommunity.com/app/550/discussions/0/3211505894112076025 and others
- Docker Hub, left4devops/l4d — https://hub.docker.com/r/left4devops/l4d
- MASQUE overview and RFC 9298/9297/9221 summaries — https://instatunnel.substack.com/p/masque-the-http3-tunneling-protocol ; https://muonics.com/rfc/wit/masque
- SitePoint and SSD Nodes explainers on DERP/NAT — https://www.sitepoint.com/tailscale-peer-relays-nat-traversal-derp/

- Microsoft Dev Blogs, "MSIX Per-User vs All Users" — https://devblogs.microsoft.com/insidemsix/msix-per-user-vs-all-users/
- Tauri v1 updater guide — https://v1.tauri.app/v1/guides/distribution/updater ; tauri-plugin-updater crate source — https://docs.rs/crate/tauri-plugin-updater/2.12.0/source/src/config.rs
- Quinn-based crate source showing datagram API — https://docs.rs/crate/majra/latest/source/src/quic.rs ; noq README — https://docs.rs/crate/noq/latest/source/README.md
- System.Net.Quic datagram design proposal (not a shipped-API confirmation) — https://gist.github.com/rzikm/f77fe0a8832fbc165d5512de4a5b5f8c
- Desktop framework comparisons (conflicting numbers, not relied on) — https://tech-insider.org/tauri-vs-electron-2026/ ; https://dasroot.net/posts/2026/03/tauri-vs-electron-rust-cross-platform-apps/
- VPS egress comparisons (re-verify on provider sites) — https://apicalculators.com/blog/cloud-vps-cost-comparison-2026 ; https://egresscost.com/vultr ; https://www.techplained.com/cheapest-vps-providers
- IETF, RFC 8252 / BCP 212, "OAuth 2.0 for Native Apps" — https://www.rfc-editor.org/info/bcp212
- Device authorization grant, vendor description (not the RFC text) — https://docs.nscale.com/docs/cli/device-authorization ; RFC 8628 reference — https://datatracker.ietf.org/doc/html/rfc8628
- IAP Desktop privacy notes (DPAPI-protected refresh token) — https://github.com/GoogleCloudPlatform/iap-desktop/wiki/Privacy
- itzg/mc-monitor — https://www.github.com/itzg/mc-monitor ; itzg/minecraft-bedrock-server — https://hub.docker.com/r/itzg/minecraft-bedrock-server ; Terraria community image — https://hub.docker.com/r/passivelemon/terraria-docker
- OpenTelemetry Rust status (copies disagree; confirm on opentelemetry.io) — https://openobserve.ai/opentelemetry/rust.md ; https://github.com/step-security-bot/open-telemetry_opentelemetry-rust
- Velopack usage examples — https://github.com/Kenshin9977/AudioWinFix ; https://github.com/Negative-Star-Innovators/AgentRedactor

**Follow-up:** replace secondary sources with Mojang/minecraft.wiki, RFC Editor texts, and Steam/Valve documentation before any [R] item is promoted to [V].

### Sources for RQ-13 (accessed 2026-10-17)

Read and used — see §5 RQ-13.1 for how each was used and for what it does **not** establish. Added 2026-10-17 for T-002:

- Minecraft Wiki, "server.properties" — https://minecraft.wiki/w/Server.properties (community; **the default files shipped with Java 26.3 and BDS 1.26.52.3**, both editions, in full — the basis for §2 and §2.1)
- Terraria Wiki, "Server" — https://terraria.wiki.gg/wiki/Server (community; TCP 7777, `serverconfig.txt`, uPNP, console commands, standalone download URL)
- Left4DevOps, `l4d2-docker` README — https://github.com/Left4DevOps/l4d2-docker (community, actively maintained; builds images for **both** L4D and L4D2; per-player IP identification, game+RCON sharing 27015 across TCP/UDP, NET_CON_PORT telnet console, join methods)

**Attempted and NOT reachable on 2026-10-17:**

- `https://developer.valvesoftware.com/wiki/Source_Dedicated_Server` and `/wiki/RCON` — both return an **Anubis anti-bot interstitial**, so Valve's pages could not be read. The `[V]` labels on the Source port rows in §2 therefore **rest on a previous session's reading (2026-10-16) and could not be re-verified today.** A search-engine snippet reproducing Valve's text ("27015 TCP/UDP (game transmission, pings and RCON)… 27020 UDP…") is consistent with the recorded figures but is a snippet, not a reading.
- `https://raw.githubusercontent.com/Left4DevOps/l4d/master/README.md` — 404. The `l4d2-docker` project covers Left 4 Dead as well, so this is not a gap.

- Microsoft Learn, "Getting Started with Bedrock Dedicated Server" — https://learn.microsoft.com/en-us/minecraft/creator/documents/bedrockserver/getting-started (vendor documentation; firewall prompt, loopback exemption, ports, folder layout, console commands)
- Microsoft Learn, "Bedrock Dedicated Server Properties" — https://learn.microsoft.com/en-us/minecraft/creator/documents/bedrockserver/server-properties (vendor documentation; the authoritative BDS key set, incl. the absence of RCON/query)
- Minecraft Wiki, "Bedrock Dedicated Server" — https://minecraft.wiki/w/Bedrock_Dedicated_Server (community; distribution form, RCON limitation, console/mobile constraint, versions)
- Minecraft Wiki, "server.jar" — https://minecraft.wiki/w/Server.jar (community; content-addressed download path, `eula.txt` behaviour, `--initSettings`, generated files, MSMP introduction)
- Minecraft Wiki, "Server/Requirements" — https://minecraft.wiki/w/Server/Requirements (community; JRE version history)
- Minecraft Wiki, "Version_manifest.json" — https://minecraft.wiki/w/Version_manifest.json (community; manifest hosts and the `sha1` field)

**Attempted and NOT reachable from the research environment on 2026-10-17** (timeouts and transport errors). Nothing from these was used, and no substitute was read in their place:

- https://www.minecraft.net/en-us/download/server/bedrock — Bedrock download page; **blocks SRV-002 for Bedrock**
- https://www.minecraft.net/en-us/download/server/java — Java server download page
- https://www.minecraft.net/en-us/eula — **READ 2026-10-17** (`aka.ms/MinecraftEULA`, HTTP 200, 35,444 bytes, redirects here). Full findings in §5 RQ-13.1b
- https://www.minecraft.net/en-us/usage-guidelines — **READ 2026-10-17** (HTTP 200, 407,376 bytes). Contains the "Servers and hosting" section and the Essential guidelines that `SRV-012`–`SRV-017` derive from
- https://www.minecraft.net/en-us/usage-guidelines — not read
- https://aka.ms/MinecraftEULA — not read

**Still to read:** the EULA and Usage Guidelines (owner or counsel — T-027); the MSMP documentation (Java server management, relevant to ADR-014); a reachable copy of the BDS download page and any published checksum for the BDS artifact (T-024); the Minecraft Server Management Protocol specification.

Nothing has been established for Terraria, Left 4 Dead, or Left 4 Dead 2 in this section. §2 remains the only source for those, and it is `[R]`.
