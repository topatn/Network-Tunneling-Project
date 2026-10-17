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
| Managed game server acquisition and supervision | Partial (§5 RQ-13.2, §5 RQ-13.3 — desk research only; AS-11/E-19 unread) |

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

**Could not be read — the gating question is unresolved.** `minecraft.net` (both server download pages, `/en-us/eula`, `/en-us/usage-guidelines`) and `aka.ms/MinecraftEULA` **timed out or returned transport errors on every attempt** from this environment. The EULA and Usage Guidelines text was therefore **not read**, and no substitute was used in its place. **AS-11 and E-19 remain unverified.** Nothing in this document asserts that automated acquisition is permitted.

#### RQ-13.2 Java Edition (`server.jar`)

- [V] Distributed by Mojang Studios, free of charge, licensed under the EULA linked from `minecraft.net/en-us/eula` (whose text I did not read).
- [V] Object store is **content-addressed by SHA-1**: observed download path form is `piston-data.mojang.com/v1/objects/<sha1>/server.jar`, e.g. release `26.3` at `.../objects/33680f5f2ac32864d6d7cf5e56a705fdb3e05f4c/server.jar`.
- [V] `version_manifest_v2.json` at `piston-meta.mojang.com` carries a **`sha1` field per version entry** (documented field list). HTTPS throughout.
- [X] **Not confirmed:** that the manifest was actually fetched and that a manifest-sourced SHA-1 verifies for the `server.jar` object specifically (the manifest hashes the version JSON, not necessarily the server jar). **E-15 must verify the real check end to end before `SRV-002` can be claimed.**
- [V] **A JRE is required, and the requirement moves with game versions** — JRE 8 historically, 16 from 1.17, 17 from 1.18, **21 from 1.20.5**. The page I read states requirements for `26.3` but does not restate the minimum, so the exact floor for the current release is [X].
- [A] **Consequence:** provisioning Java edition means acquiring *two* artifacts — the server jar **and a JRE** — from two different vendors, both verified, neither shipped in our installer. This roughly doubles the supply-chain surface of `SRV-002` and must be budgeted in `SRV-001` scope and disk footprint.
- [V] Legal acceptance is an explicit user act: `eula.txt` is generated with `eula=false`, the process exits, and `eula` must be set to `true` to start.
- [V] `--initSettings` creates `server.properties` and `eula.txt` **without starting the server** — a clean hook for provisioning.
- [V] Generated files: `server.properties`, `ops.json`, `whitelist.json`, `banned-players.json`, `banned-ips.json`, `logs/latest.log` plus gzipped rotations, world directory named by `level-name`, `libraries/`, `versions/`.
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
| `management-server-secret` | generated and **written to `server.properties`** if empty | Exactly 40 alphanumeric characters. Presented as a bearer token or via `Sec-WebSocket-Protocol: minecraft-v1,<secret>`. Unauthorized requests get 401. |
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
| ADR-007 | Guest model: guests install client (G1) vs connect to relay address (G2) | Open | 2026-10-16 | n/a |
| ADR-008 | Rust control plane (`axum`) + PostgreSQL (`sqlx`); relay state in memory | **Accepted 2026-10-17** by the product owner | 2026-10-16, recorded 2026-10-17 | Medium |
| ADR-009 | System-browser OAuth (PKCE) + OS credential store; identity provider open | Proposed | 2026-10-16 | Medium (flow), none (provider) |
| ADR-010 | `tracing` + optional OTLP; no default client telemetry | Proposed | 2026-10-16 | Medium |
| ADR-011 | Compatibility test levels L0–L2; "supported" requires L2 | Proposed | 2026-10-16 | Medium |
| ADR-012 | Host-local game server provisioning runs as an engine-supervised child process on the host's PC; never operator-hosted | Proposed | 2026-10-17 | Low (no evidence; E-15–E-19) |
| ADR-013 | First provisioning cut is **Minecraft Java Edition only**. Bedrock deferred pending a published integrity mechanism; Terraria, L4D and L4D2 stay manual | **Decided 2026-10-17** | 2026-10-17 | Medium. Based on documented behaviour, not on a run |
| ADR-014 | Treatment of a generated game-server credential that the server must read from its configuration file | Open, **narrowed twice** — now Java-only (Bedrock has no RCON), and RQ-13.4 offers a better answer | 2026-10-17 | n/a |
| ADR-015 | Prefer MSMP over RCON for Java server management; never enable RCON; let the server generate its own MSMP secret; pass the TLS keystore password by environment variable | Proposed | 2026-10-17 | Medium (documented behaviour read; not exercised) |
| ADR-017 | `sqlx migrate` owns `migrations/`; no separate migration library | **Accepted 2026-10-17** | 2026-10-17 | Medium-high; tool not yet executed, T-033 must verify |

Each ADR still needs full context, options, rationale, and consequences sections. Not yet written. **ADR-016 added 2026-10-17** (see below).

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

**Not settled here.** Identity provider (ADR-009) and the admin authentication model (SEC-018) are untouched; SEC-018 is still undesigned.

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
| E-15 | Minecraft Java server artifact: official channel, terms, unattended download, integrity check, standard-user install, required runtime | ADR-013, SRV-001, SRV-002 |
| E-16 | Minecraft Bedrock server artifact: same questions, plus the license-acceptance step and the settings that must be generated | ADR-013, SRV-001, SRV-002 |
| E-17 | Supervising a headless server process: start, graceful stop, bounded crash restart, log capture, exit-code handling | ADR-012, SRV-007 |
| E-18 | Update, rollback, world backup/restore, and per-server disk/CPU/memory footprint | SRV-008, SRV-009 |
| E-19 | Licensing and end-user terms for the host operating these servers, plus a clean standard-user VM footprint check | SRV-010, AS-11 |

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
- https://www.minecraft.net/en-us/eula — **the EULA. Not read. AS-11 / E-19 remain unverified**
- https://www.minecraft.net/en-us/usage-guidelines — not read
- https://aka.ms/MinecraftEULA — not read

**Still to read:** the EULA and Usage Guidelines (owner or counsel — T-027); the MSMP documentation (Java server management, relevant to ADR-014); a reachable copy of the BDS download page and any published checksum for the BDS artifact (T-024); the Minecraft Server Management Protocol specification.

Nothing has been established for Terraria, Left 4 Dead, or Left 4 Dead 2 in this section. §2 remains the only source for those, and it is `[R]`.
