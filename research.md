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
| Terraria | TCP 7777 | [R] | One wiki page also says to open TCP and UDP 7777 on cloud hosts; other guides say TCP only. UDP need is unresolved. [X] |
| Minecraft Java | `server-port` TCP 25565; `query.port` UDP 25565 (only with `enable-query=true`); `rcon.port` TCP 25575 (only with `enable-rcon=true`) | [R] for the port numbers; **[V] for the property names** (`enable-rcon`, `rcon.password`, `rcon.port`, `enable-query`, `query.port` confirmed, read 2026-10-17) | Port numbers still rest on an Arch wiki mirror and hosting providers, not Mojang — T-002 still open. The property names are confirmed. Query and RCON are optional and RCON must not be exposed by default. **New since 1.21.9: the Minecraft Server Management Protocol (MSMP), a management API with mandatory client authentication and TLS on by default — an unexamined alternative to RCON** |
| Minecraft Bedrock | UDP; `server-port` 19132 (IPv4), `server-portv6` 19133 (IPv6) | **[V] for the port defaults and the property set** (Microsoft's official BDS properties reference, read 2026-10-17); [R] for RakNet naming | **Corrected 2026-10-17.** `enable-lan-visibility` defaults to **true** and per Microsoft's own documentation "will cause the server to bind to the default ports (19132 and 19133) even when `server-port` and `server-portv6` have non-default values" — previously [R], now [V] as documented behavior (actual runtime binding still [X], E-16). A previously cited **`server-udp-ports` setting does not appear in the official property reference** and should be treated as unverified. BDS has **no RCON and no query**. **Console players on Xbox/PlayStation/Switch can only connect via custom DNS or LAN**, so they cannot use a relay address (AS-2 answered **no**) |
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

I gathered **no external evidence** for this question, so everything here is reasoning [A], not research. Candidates: Rust (axum-style) service; TypeScript on Node; C# on ASP.NET; Go.

Criteria: shares protocol and token-validation code with the Rust relay and client engine; operational simplicity for a small team; maturity of Postgres drivers and migration tooling; ability to run the control plane as one deployable.

Reasoned recommendation (ADR-008, proposed): Rust control plane sharing a protocol crate with the engine and relay, PostgreSQL for durable state, relay session state in memory only. Confidence: **low**. Before accepting it I still need to check maintenance status of the specific web framework, database driver, and migration tool, and to weigh team familiarity. If the team is stronger in TypeScript, a TypeScript control plane with a shared schema package is a reasonable alternative; the relay stays Rust either way.

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
| ADR-008 | Rust control plane + PostgreSQL; relay state in memory | Proposed (reasoning only) | 2026-10-16 | Low |
| ADR-009 | System-browser OAuth (PKCE) + OS credential store; identity provider open | Proposed | 2026-10-16 | Medium (flow), none (provider) |
| ADR-010 | `tracing` + optional OTLP; no default client telemetry | Proposed | 2026-10-16 | Medium |
| ADR-011 | Compatibility test levels L0–L2; "supported" requires L2 | Proposed | 2026-10-16 | Medium |
| ADR-012 | Host-local game server provisioning runs as an engine-supervised child process on the host's PC; never operator-hosted | Proposed | 2026-10-17 | Low (no evidence; E-15–E-19) |
| ADR-013 | First provisioning cut is **Minecraft Java Edition only**. Bedrock deferred pending a published integrity mechanism; Terraria, L4D and L4D2 stay manual | **Decided 2026-10-17** | 2026-10-17 | Medium. Based on documented behaviour, not on a run |
| ADR-014 | Treatment of a generated game-server credential that the server must read from its configuration file | Open, **narrowed twice** — now Java-only (Bedrock has no RCON), and RQ-13.4 offers a better answer | 2026-10-17 | n/a |
| ADR-015 | Prefer MSMP over RCON for Java server management; never enable RCON; let the server generate its own MSMP secret; pass the TLS keystore password by environment variable | Proposed | 2026-10-17 | Medium (documented behaviour read; not exercised) |

Each ADR still needs full context, options, rationale, and consequences sections. Not yet written.

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

Read and used — see §5 RQ-13.1 for how each was used and for what it does **not** establish:

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
