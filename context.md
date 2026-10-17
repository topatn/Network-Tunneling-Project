# context.md — Product Context and Boundaries

**Status:** DRAFT. Canonical for product context and release boundaries.
**Working title:** Game Tunnel Tool (name not decided).
**Last updated:** 2026-10-17

Items marked **[Depends on ADR-007]** assume the default guest model (guests connect to a relay address with the stock game). That decision is still open; see `research.md` §5.

## 1. Product vision

A Windows desktop app that lets a person host a multiplayer game server on their own PC and let friends join over the internet without router port forwarding, a public IP address, or NAT configuration. The host picks a game profile, points it at a local server, starts a tunnel, and shares a connection address.

## 2. Target users

| User | Description | Needs |
|---|---|---|
| Host | Plays on a home or school connection, runs a game server on their PC, often behind a router they cannot or will not configure (including carrier-grade NAT) | One-click tunnel, clear status, control over who can join |
| Guest | A friend of the host using the stock game client | A single address to enter; no setup beyond the game |
| Operator | Runs the control plane and relays | Cost control, abuse controls, observability |

## 3. Problems being solved

1. Port forwarding is technical, router-specific, and impossible behind some ISP networks (the research notes carrier-grade NAT as a common cause; see `research.md` §4).
2. Existing workarounds add friction: asking guests to install VPN-style software, or renting hosting.
3. Hosts cannot tell why a friend cannot join: tunnel down, game server not listening, or a game-level issue.

## 4. Core user journeys (summary)

Canonical detail lives in `user-flows.md`.

1. First launch, sign-in, choose a game profile.
2. Configure or detect the local server; start the tunnel.
3. Share the address or invitation; a guest joins.
4. Monitor status; stop the session and revoke access.
5. Recover from network drops, blocked UDP, or an unreachable relay.

## 5. Goals

- G-1: Host-to-guest connectivity without port forwarding on common networks.
- G-2: TCP and UDP support, because three of the five initial games are UDP-first (`research.md` §2).
- G-3: Honest status that separates tunnel health, local server reachability, and whether players are actually connected.
- G-4: Security by design: no silent exposure of local services, scoped and expiring access, abuse controls.
- G-5: Costs the operator can understand and cap.
- G-6: Modular game profiles so games can be added without engine changes.
- G-7: A host can reach a working, tunneled game server without installing, licensing, or configuring the game server themselves. Host-local provisioning only (see §7); no operator-hosted servers.

## 6. Non-goals

- NG-1: Managing or hosting game servers **on operator infrastructure**. *Amended 2026-10-17:* provisioning and supervising a game server on the **host's own PC** is now in scope (§7, G-7). The tool still does not run, host, or bill for game servers on its own servers.
- NG-2: A public server browser or matchmaking service.
- NG-3: Bypassing game-level authentication, bans, or anti-cheat. The tool must not weaken them.
- NG-4: Covert or persistent operation, security-control evasion, or access to third-party machines.
- NG-5: Making the game protocol end-to-end encrypted in the initial release.
- NG-6: Guaranteeing compatibility with any game beyond its documented, tested scope.

## 7. Initial release boundaries

In scope:
- Windows 10/11, 64-bit, per-user install.
- Profiles for Terraria, Minecraft Java, Minecraft Bedrock, Left 4 Dead, Left 4 Dead 2, each with a **documented support scope** (direct connect by address; lobby and matchmaking flows only if experiments confirm them).
- Relay-first connectivity. **[Depends on ADR-007]** Guests join via a relay-allocated address.
- Host accounts, tunnel lifecycle, invitations, quotas, basic abuse controls.
- **Host-local game server provisioning (added 2026-10-17, G-7).** For games whose acquisition path has been verified by experiment, the app downloads the official server artifact, verifies it, generates a configuration, and supervises it as a child process on the host's own PC. **First cut is limited to Minecraft Java Edition** (ADR-013; requirements `SRV-001`–`SRV-011`). **Minecraft Bedrock Edition was removed from the first cut on 2026-10-17:** no integrity mechanism for its artifact could be established, so `SRV-002` cannot be met and `SRV-003` correctly excludes it (`research.md` RQ-13.3, E-16). Bedrock provisioning is **deferred**, not rejected, and returns if a published checksum or signature becomes available. Terraria, Left 4 Dead, and Left 4 Dead 2 keep manual local configuration until their acquisition path is verified; a host may always configure their own server by hand instead.

Explicitly **not** in the initial release (deferred):
- Direct peer-to-peer upgrade (pending experiment E-7).
- Guest-side client and end-to-end host-to-guest encryption (ADR-004 / ADR-007).
- macOS and Linux clients.
- Windows service mode.
- Steam lobby/matchmaking integration unless experiment E-4 passes.

## 8. Constraints

Functional: explicit local destinations only; per-mapping protocol and port; revocable access.
Nonfunctional: no administrator rights in normal use; no telemetry from the client by default; secrets never in plain files or logs; operator-visible cost drivers.
Technical: UDP must be relayed faithfully (research RQ-3); relay never dials client-chosen destinations (ADR-003).

Constraints added for host-local provisioning (2026-10-17):

- A provisioned server runs as the current user, from a directory under the user's profile. No administrator rights, no service installation, no writes outside that directory (SEC-012, INST-001, `SRV-010`).
- Only artifacts from official sources with a recorded verification method are downloaded, and no artifact runs before verification (`SRV-002`).
- The provisioned server's listen port is host-local configuration. It is set from the tunnel mapping but is **never uploaded** (INV-1, ADR-003, SEC-006, `SRV-005`).
- Query and RCON are off by default on a provisioned server **where the game has them** — Java edition only; Bedrock supports neither (`research.md` RQ-13.3) — and enabling RCON is an explicit host action with a generated secret (GAME-008, `SRV-004`).
- The tunnel boundaries are unchanged. Provisioning adds a supervised local process; it does not give the relay or the control plane any new destination.

## 9. Business logic boundaries

- The **control plane** decides who may create, join, view, and terminate tunnels, assigns relays, enforces quotas, and issues short-lived credentials.
- The **relay** forwards bytes for an allocation it has been authorized to serve. It does not make policy decisions beyond the allocation's limits.
- The **client engine** decides what local address and port a mapping may reach. Only the host's local configuration defines destinations.
- The **game server** decides game-level admission (passwords, whitelists, bans).
- The **server manager** (inside the client engine, never the UI) may download, verify, configure, start, stop, and update a game server process on the host's machine. It never exposes that process beyond the mappings the host configured, and it never changes a tunnel destination. It is unavailable to the relay and to the control plane.

## 10. Assumptions requiring validation

| ID | Assumption | Validation |
|---|---|---|
| AS-1 | A relay-allocated public address works with the stock client for each game | E-1 to E-5 |
| AS-2 | Console and mobile Bedrock players can connect to an arbitrary address | **Answered 2026-10-17: NO.** Xbox, PlayStation and Switch Bedrock players "can only connect through a custom DNS or a LAN connection" (`research.md` RQ-13.3, Microsoft's own BDS documentation). They cannot use a relay-allocated IP:port. Console and mobile are out of scope for the Bedrock profile; GAME-003 corrected |
| AS-3 | IP-allowlisting guests at the relay is workable despite shared and changing addresses (CGNAT, mobile) | New experiment E-13 |
| AS-4 | QUIC datagrams carry real game UDP traffic within MTU limits | E-6 |
| AS-5 | Relay-first latency is acceptable for these games in target regions | E-7, E-10 |
| AS-6 | Target player regions and audience size | Product decision; not yet made |
| AS-7 | Terraria needs only TCP | E-1 |
| AS-8 | The official Minecraft Java server artifact (and a JRE for it) can be downloaded, verified, configured, and run by a standard user with no manual steps | E-15. **Bedrock removed from the first cut**: no integrity mechanism established, so E-16 must first find a published checksum or signature (ADR-013) |
| AS-9 | Both artifacts can be updated and rolled back without losing the world | E-18 |
| AS-10 | Supervising a headless server process (start, crash restart, log capture) is reliable enough to ship | E-17 |
| AS-11 | Licensor terms and end-user terms permit the host to download and operate these servers on their own machine | E-19 — **ANSWERED from primary source 2026-10-17** (`research.md` §5 RQ-13.1b). The EULA and Usage Guidelines were read in full: **hosting is explicitly permitted**, including charging for access, and building tools and services is permitted provided they do not appear official. **Six binding constraints were found and are now requirements** (`SRV-012`–`SRV-017`); two of them — guest admission must be limited to genuine paid-for owners (C1), and access must not be gated on out-of-game content (C2) — bear directly on design, and **C1 has no published verification API**. **Owner or legal counsel must still confirm that our implementation complies; this is not legal advice** |
| AS-12 | A guest can still be identified and joined normally through a relay even though the game server sees a host-local source address | E-3, E-5. **Raised 2026-10-17:** Source-engine games identify players by IP (`research.md` §2.2), and our engine dials the game server from a single local address, so all guests may collapse into one identity and game-level IP banning may stop working. Joining is assumed unaffected but **unverified** |

## 11. Success metrics

Targets are **not set**. Each needs a baseline measurement first.

| Metric | Definition | Target |
|---|---|---|
| Time to first successful join | App launch to a guest in-game, first session | TBD after usability test |
| Time to first successful join, provisioned server | Install to a guest in-game including artifact download and first server start | TBD after usability test |
| Tunnel start success rate | Starts reaching Active / attempts | TBD after E-6/E-7 |
| Relay latency overhead | Added round-trip vs direct, per game | TBD after benchmark |
| Compatibility | Games at L2 pass (`research.md` ADR-011) | 5 of 5 aspired; reported per game |
| Cost per active hour | Relay egress and compute | TBD after E-10 |
| Support burden | Share of sessions needing troubleshooting | TBD |

## 12. Related documents

`idea.md`, `research.md`, `architecture.md`, `requirements.md`, `user-flows.md`, `data-model.md`, `contracts.md`.
