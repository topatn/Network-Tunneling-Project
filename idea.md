# idea.md — Product Brief

**Status:** DRAFT. Must stay aligned with `context.md`, which is canonical on scope.

## Concept

A Windows desktop app that makes a self-hosted game server joinable by friends over the internet without port forwarding. The host starts a tunnel; the app connects out to a relay; friends connect to an address the relay provides.

## Value proposition

- Works where port forwarding does not (for example, behind carrier-grade NAT).
- Carries both TCP and UDP, so UDP-first games are in scope.
- Shows whether the *tunnel*, the *local server*, and *players* are each working, instead of one vague "connected" light.
- Host-controlled access with expiring, revocable invitations.

## Key differentiators (hypotheses, not validated)

- Per-game profiles with explicit, validated port mappings and a documented support scope.
- Honest compatibility reporting: a game is "supported" only after a real-client join test passes.
- No administrator rights and no default client telemetry.

## Initial game support

Terraria, Minecraft Java, Minecraft Bedrock, Left 4 Dead, Left 4 Dead 2. Scope per game is *direct connect by address* until experiments show more. Status of each is in `research.md` §2 (nothing confirmed yet).

## Major risks

| Risk | Note |
|---|---|
| Relay egress cost | Dominant cost driver; no measurements yet (E-10) |
| Public relay endpoints invite abuse | Allowlisting and quotas needed (AS-3, E-13) |
| UDP game traffic over QUIC datagrams | Unproven (E-6) |
| Steam lobby flows for L4D/L4D2 | Unknown whether they work through a relay (E-4) |
| Relay can read game traffic in the default guest model | Disclosed; encryption option deferred (ADR-004) |

## Future opportunities

Direct peer-to-peer upgrade, guest client with end-to-end encryption, more game profiles, macOS and Linux, regional relay expansion. None is committed.
