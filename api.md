# api.md — API Route Inventory

**Status:** DRAFT. Inventory only. Request/response definitions, errors, and rules are in `contracts.md`; they are not repeated here, so they cannot drift.

**Note 2026-10-17:** this file previously also pointed at schemas in `docs/contracts/`. That directory **does not exist in this repository**, so for now `contracts.md` is the only wire-format source of truth. See `contracts.md`'s status paragraph.

Legend — Auth: **user** = access token; **none** = unauthenticated; **relay** = relay credential; **admin** = separate admin authentication (not yet defined). Idem = supports `Idempotency-Key`.

## User routes (`/v1`)

| # | Method | Path | Auth | Permission check | Idem | Request schema | Success | Contract |
|---|---|---|---|---|---|---|---|---|
| 1 | POST | `/v1/auth/exchange` | none | redirect URI allow-list, PKCE verifier (`S256` only, ADR-018) | no | provisional (provider-dependent) | 200 | §5.1 |
| 2 | POST | `/v1/auth/refresh` | refresh token | token valid, not reused | no | `{refresh_token}` | 200 | §5.2 |
| 3 | POST | `/v1/auth/logout` | user | own session | no | none | 204 | §5.3 |
| 4 | GET | `/v1/me` | user | self | no | none | 200 | §5.4 |
| 5 | GET | `/v1/profiles` | user | any user | no | none | 200 | §5.5 |
| 6 | POST | `/v1/tunnels` | user | active account, quota | yes | `create-tunnel-request` | 201 `tunnel` | §5.6 |
| 7 | GET | `/v1/tunnels` | user | own only | no | none | 200 | §5.7 |
| 8 | GET | `/v1/tunnels/{id}` | user | owner | no | none | 200 `tunnel` | §5.7 |
| 9 | POST | `/v1/tunnels/{id}/start` | user | owner, relay capacity, quota | yes | none | 200 `start-tunnel-response` | §5.8 |
| 10 | POST | `/v1/tunnels/{id}/token` | user | owner, live allocation | no | none | 200 | §5.9 |
| 11 | POST | `/v1/tunnels/{id}/stop` | user | owner | natural | none | 200 `tunnel` | §5.10 |
| 12 | POST | `/v1/tunnels/{id}/invitations` | user | owner, quota | yes | `create-invitation-request` | 201 `create-invitation-response` | §5.11 |
| 13 | GET | `/v1/tunnels/{id}/invitations` | user | owner | no | none | 200 | §5.12 |
| 14 | DELETE | `/v1/invitations/{id}` | user | owner of the tunnel | natural | none | 204 | §5.12 |
| 15 | GET | `/v1/tunnels/{id}/admissions` | user | owner | no | none | 200 | §5.13 |
| 16 | DELETE | `/v1/admissions/{id}` | user | owner of the tunnel | natural | none | 204 | §5.13 |
| 17 | GET | `/v1/tunnels/{id}/usage` | user | owner | no | none | 200 | §5.14 |

## Guest route

| # | Method | Path | Auth | Permission check | Request schema | Success | Contract |
|---|---|---|---|---|---|---|---|
| 18 | POST | `/v1/invitations/claim` | none | valid, unexpired, unrevoked, unexhausted secret; strict rate limit | `claim-invitation-request` | 200 `claim-invitation-response` | §6 |

Rows 15, 16, and 18 depend on guest source-IP admission (experiment E-13).

## Internal routes (`/internal/v1`)

| # | Method | Path | Auth | Permission check | Contract |
|---|---|---|---|---|---|
| 19 | GET | `/internal/v1/relays/{relay_id}/state` | relay | relay may access only its own id | §7 |
| 20 | POST | `/internal/v1/relays/{relay_id}/report` | relay | relay may access only its own id | §7 |

## Admin routes (`/admin/v1`)

Not defined. See `contracts.md` §9 (open).

## Non-HTTP contracts

| Contract | Where |
|---|---|
| Engine ↔ relay tunnel protocol and message schemas | `contracts.md` §10, `contracts/tunnel-*.schema.json` |
| TCP stream and UDP datagram framing | `contracts.md` §11 |
| State machines | `contracts.md` §12 |
| Engine ↔ UI interface | `contracts.md` §13 |
