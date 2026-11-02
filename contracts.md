# contracts.md — Component and Protocol Contracts

**Status:** DRAFT v0.1. Authoritative for API and component contracts. Route inventory: `api.md`. Entities: `data-model.md`.
**Last updated:** 2026-10-19

**Verification status — updated 2026-10-19 (T-031).** Earlier drafts (corrected 2026-10-17) noted that the 11 JSON Schemas and `contracts/examples.json` cited by this document, `api.md`, `data-model.md` §7, and `tasks.md` T-031 had not yet been written. **T-031 has now authored all 11 draft 2020-12 JSON Schemas in `contracts/*.schema.json` and 29 positive and negative conformance vectors in `contracts/examples.json`, and implemented `crates/protocol` (`gtt-protocol`).**

What is verified locally (2026-10-19):

- **All 11 JSON Schemas in `contracts/*.schema.json`** are present, validated for draft 2020-12 structural well-formedness, and cross-checked against every example vector in `contracts/examples.json` by `crates/protocol/tests/protocol.rs`.
- **All 29 vectors in `contracts/examples.json`** (covering every schema, unknown-field rejection on requests, `INV-1` local-destination field rejection at top level and nested objects, `INV-7` terminal `stopped_at` consistency, duplicate mapping keys/indexes, control characters, and numeric/string bounds) pass or fail as specified under `cargo +stable-x86_64-pc-windows-gnu test -p gtt-protocol` (**8 test functions, 0 failures**).
- **`uuid` and RFC 3339 UTC `date-time` string formats are explicitly enforced** by `gtt-protocol::validate` (including leap-year calendar validation for `date-time`), closing the open question from 2026-10-17.

All numeric limits, timeouts, and lifetimes marked *(proposal)* remain starting points that `requirements.md` fixes as Milestone M1 experiments complete. Items depending on guest source-IP admission carry **[E-13]**.

## 1. Scope and conventions

- Control plane HTTP API: JSON over HTTPS, base path `/v1`. Internal relay API under `/internal/v1`. Admin API under `/admin/v1` (open, §9).
- Identifiers are UUIDs. Timestamps are RFC 3339 in UTC. Field names are `snake_case`.
- Requests and responses with `additionalProperties: false` reject unknown fields on **requests**; clients must **ignore** unknown fields on responses (§14).
- Secrets (`allocation_token`, invitation secret, refresh token) are returned only in responses marked `Cache-Control: no-store` and are never logged.
- **The control plane never accepts or stores a local destination** (INV-1). Any request field resembling a local address or port is rejected by schema.

## 2. Authentication and tokens

| Token | Holder | Form | Lifetime | Purpose |
|---|---|---|---|---|
| Access token | Desktop app | Signed bearer token in `Authorization` | Short *(proposal)* | Calls to user routes |
| Refresh token | Desktop app, in OS credential store | Opaque random secret, hash stored (`auth_sessions`) | Rotating, bounded *(proposal)* | Obtain new access tokens; reuse of a rotated token revokes the family |
| Allocation token | Host engine → relay | Signed token | Short *(proposal)* | Authorize one tunnel session at one relay |
| Invitation secret | Host → guest (out of band) | Opaque random, ≥128 bits of entropy, base64url | Expiring, revocable | Claim guest admission |
| Relay credential | Relay → control plane | Per-relay, revocable credential (mTLS vs signed token: **open**) | Rotated | Internal API |

Signing algorithm and key format are an implementation decision requiring a security review; the contract fixes only these claims.

**Allocation token claims:** `iss`, `aud` (relay id), `sub` (tunnel id), `aid` (allocation id), `mappings` (index, mapping_key, protocol, public_port), `iat`, `exp`, `jti`, and a key id header (`kid`) for rotation. The relay verifies signature, `aud`, `exp`, and `jti` uniqueness (in-memory replay cache until `exp`) and then accepts the session.

### Authorization matrix

| Operation | Owner | Other user | Guest (admitted) | Relay | Admin |
|---|---|---|---|---|---|
| Create / list tunnels | yes (own) | no | no | no | via admin API |
| Start / stop / view tunnel | yes | no (`not_found`) | no | no | via admin API |
| Create / revoke invitation, list admissions | yes | no (`not_found`) | no | no | via admin API |
| Claim invitation | n/a | anyone with the secret | n/a | no | no |
| Report health/usage, fetch state | no | no | no | own relay only | no |
| Suspend user / terminate tunnel | no | no | no | no | yes |

Cross-owner access returns `not_found`, never `forbidden`, so identifiers are not confirmable.

## 3. Error model

All error responses use `ErrorResponse` (`contracts/error.schema.json`):

```json
{"error":{"code":"validation_failed","message":"Request body is invalid.","request_id":"req_example","details":[{"field":"name","issue":"must be 1-64 characters"}]}}
```

| `code` | HTTP | Meaning |
|---|---|---|
| `unauthenticated` | 401 | Missing, invalid, or expired credentials |
| `forbidden` | 403 | Authenticated but not allowed (e.g. suspended account) |
| `not_found` | 404 | Resource absent or not visible to the caller |
| `validation_failed` | 400 | Schema or domain validation failed; `details` lists fields |
| `conflict` | 409 | State conflict (e.g. start on a stopped tunnel) |
| `idempotency_mismatch` | 422 | Same `Idempotency-Key` with a different request body |
| `rate_limited` | 429 | Includes `Retry-After` |
| `quota_exceeded` | 403 | Account quota reached (limits *(proposal)*) |
| `invitation_invalid` | 400 | Expired, revoked, exhausted, or unknown secret; one code for all, so a guesser learns nothing |
| `relay_unavailable` | 503 | No healthy relay with capacity |
| `client_upgrade_required` | 426 | Client below the minimum supported version |
| `internal` | 500 | Unexpected; `request_id` for support |

Error messages never echo secrets or internal identifiers.

## 4. Timeouts, retries, rate limits

| Item | Rule (all *(proposal)*) |
|---|---|
| Client connect / request timeout | Short bounded values; set by the client library defaults documented in `requirements.md` |
| Retry | GET and idempotent POST (with `Idempotency-Key`) retry on network errors, 429, 502/503/504 with exponential backoff and jitter, honoring `Retry-After`; at most 3 attempts |
| No retry | POST without an idempotency key; 4xx other than 429 |
| Token refresh | On 401 once; if refresh fails, return the user to sign-in |

Rate-limit classes (values to be set in `requirements.md`): **strict** for unauthenticated `POST /v1/invitations/claim` and refresh; **normal** for tunnel mutations; **lenient** for reads. Keys: per IP for unauthenticated routes, per user for authenticated routes, plus global ceilings. Limits return `rate_limited`.

## 5. User HTTP API

All routes require `Authorization: Bearer <access token>` unless stated. Success responses are JSON. Every route can also return `unauthenticated`, `rate_limited`, `client_upgrade_required`, `internal`.

### 5.1 `POST /v1/auth/exchange`
Exchange an identity-provider authorization code (PKCE) for session tokens. **Provider-dependent; shape is provisional (ADR-009).**
- Auth: none. Request: `{code, code_verifier, redirect_uri, device_label?}`. Success `200`: `{access_token, refresh_token, expires_in}`.
- Errors: `validation_failed`, `unauthenticated` (code rejected), `forbidden` (suspended).
- Checks: redirect URI must be in the registered allow-list (loopback form per RFC 8252); verifier must match.
- **PKCE challenge method is `S256` only.** A `plain` challenge is rejected with `validation_failed`; there is no fallback and no configuration to enable one. **Decided 2026-10-17 (ADR-018, accepted):** RFC 9700 §2.1.1 states *"Currently, S256 is the only such method"*, and a contract that does not name the method cannot reject `plain`. Negative test required (T-040).
- **Access tokens are not sender-constrained at launch.** Deferred deliberately per ADR-018, which the product owner accepted on 2026-10-17 — recorded as a decision with a reason rather than an omission. Revisit when the identity provider (ADR-009) is chosen; DPoP (RFC 9449) is the likely cheaper mechanism.

### 5.2 `POST /v1/auth/refresh`
- Auth: refresh token in body (no access token). Request: `{refresh_token}`. Success `200`: new access and refresh tokens (rotation). Errors: `unauthenticated`, `forbidden`. Reuse of an already-rotated token revokes the session family and returns `unauthenticated`.

### 5.3 `POST /v1/auth/logout`
- Revokes the current session family. Success `204`.

### 5.4 `GET /v1/me`
- Success `200`: `{id, display_name, status}`. No other personal data.

### 5.5 `GET /v1/profiles`
- Purpose: list profile ids and versions the server accepts. Success `200`: `{profiles:[{id, version, mappings:[{mapping_key, protocol, default_enabled}]}]}`. Contains no local ports.

### 5.6 `POST /v1/tunnels`
- Purpose: create a tunnel record (no relay allocated yet). Request: `CreateTunnelRequest`. Idempotent via `Idempotency-Key` (§8).
- Success `201`: `Tunnel` (`state: created`).
- Errors: `validation_failed` (unknown profile, mapping not in profile, protocol mismatch, duplicate keys), `quota_exceeded`, `forbidden`.
- Checks: caller active; mappings validated against the profile (INV-2); quota.

Example request:
```json
{"name":"Friday Terraria","game_profile_id":"terraria","mappings":[{"mapping_key":"game","protocol":"tcp","enabled":true}]}
```

### 5.7 `GET /v1/tunnels` and `GET /v1/tunnels/{id}`
- List own tunnels / fetch one. Success `200`: `Tunnel[]` / `Tunnel`. Errors: `not_found`. Check: owner only.

### 5.8 `POST /v1/tunnels/{id}/start`
- Purpose: pick a relay, reserve public ports, issue an allocation token. Idempotent via `Idempotency-Key`; a repeat returns the same allocation while it is live.
- Success `200` (`Cache-Control: no-store`): `StartTunnelResponse`.
- Errors: `not_found`, `conflict` (tunnel not in `created`/`stopped`-restartable state: **restart policy open**), `relay_unavailable`, `quota_exceeded`.
- Effects: `tunnels.state → active`, allocation `reserved`, audit event. Note: `active` is **lifecycle intent**, not connectivity; connectivity is reported by the engine and relay (`architecture.md` §12).

Example response:
```json
{"tunnel_id":"3f2b8c1e-5a47-4d0b-9c1e-0a6f1d2e7b44","allocation_token":"<opaque-signed-token>","allocation":{"id":"3f2b8c1e-5a47-4d0b-9c1e-0a6f1d2e7b44","expires_at":"2026-10-16T12:00:00Z","relay":{"region":"example-region","host":"relay.example.invalid","tunnel_port":4433},"ports":[{"mapping_key":"game","protocol":"tcp","public_port":20001}]}}
```

### 5.9 `POST /v1/tunnels/{id}/token`
- Purpose: issue a fresh allocation token for the current live allocation (reconnect / refresh). Success `200`: `{allocation_token, expires_at}` (`no-store`). Errors: `not_found`, `conflict` (no live allocation).

### 5.10 `POST /v1/tunnels/{id}/stop`
- Purpose: end the session, release ports, revoke admissions. Idempotent. Success `200`: `Tunnel`. Effects: tunnel `stopped`, allocation `released`, all invitations for the tunnel revoked, admissions revoked, relay told through the next state poll (§7).

### 5.11 `POST /v1/tunnels/{id}/invitations`
- Request: `CreateInvitationRequest`. Success `201` (`no-store`): `CreateInvitationResponse`; the secret is shown once. Errors: `not_found`, `conflict` (tunnel not startable/active), `quota_exceeded`, `validation_failed`.

Example request: `{"expires_in_seconds":3600,"max_claims":4}`

### 5.12 `GET /v1/tunnels/{id}/invitations`, `DELETE /v1/invitations/{id}`
- List invitations (never returns secrets) / revoke one. Revocation also revokes its admissions. Check: owner only.
- Success `200` (`GET`): the tunnel's invitations. Success `204` (`DELETE`): no body.

### 5.13 `GET /v1/tunnels/{id}/admissions`, `DELETE /v1/admissions/{id}` **[E-13]**
- List live guest admissions (source IPs shown to the owner only in masked form *(proposal)*) / revoke one.
- Success `200` (`GET`): the live admissions. Success `204` (`DELETE`): no body.

### 5.14 `GET /v1/tunnels/{id}/usage`
- Success `200`: aggregated bytes and peak flows over a requested window. Check: owner only.

## 6. Guest route (unauthenticated)

### `POST /v1/invitations/claim` **[E-13]**
- Request: `ClaimInvitationRequest` `{secret}`. Success `200`: `ClaimInvitationResponse` with the public host and ports the stock game should use, and `admitted_until`.
- Errors: `invitation_invalid`, `rate_limited`, `validation_failed`.
- **The admitted source IP is the address the server observes on the connection** (or from an explicitly configured trusted proxy header), never a client-supplied field.
- Effects: increments `claim_count`, creates or refreshes the admission for that IP.
- Known risks: IPv4/IPv6 mismatch between the claiming connection and the game connection; shared NAT; address changes on mobile. These decide whether the mechanism is viable (E-13).

Example request: `{"secret":"AbCdEfGhIjKlMnOpQrStUv"}`

## 7. Internal relay ↔ control plane API

Authenticated with the relay credential. A relay may act only on its own `relay_id`.

- `GET /internal/v1/relays/{relay_id}/state?cursor=<opaque>` — relay **pulls** changes: new or released reservations (public ports, tunnel ids, token verification keys by `kid`), admission adds/removals, relay drain flag. The relay polls; interval and resulting **revocation latency** are *(proposal)* and must be measured. Pull was chosen so relays need no inbound control port; the cost is that revocation is not instantaneous.
- `POST /internal/v1/relays/{relay_id}/report` — health, capacity, and aggregated usage per allocation per interval, flow counts. The control plane upserts `usage_records`.

If the control plane is unreachable, the relay keeps serving existing allocations until their tokens or admissions expire, and accepts no new reservations.

## 8. Idempotency

- Header `Idempotency-Key` (8–128 characters) on `POST /v1/tunnels`, `…/start`, `…/invitations`.
- The server stores key, route, and a hash of the request per user. Same key + same hash within the retention window → return the stored response. Same key + different hash → `idempotency_mismatch`. Window *(proposal)*.
- `stop` and `DELETE` routes are naturally idempotent.
- Expired keys are treated as fresh even before physical cleanup. Reuse atomically replaces only an expired response; an unexpired response cannot be overwritten. Expiry comparisons use the database statement clock, so waiting in a transaction does not extend retention. Cleanup skips locked keys and must not delete a renewed response. The retention duration remains configuration, not a newly selected policy value.

Implementation note (T-042, 2026-10-29): `POST /v1/tunnels` serializes creates for the same owner and writes the tunnel, mappings, creation audit event, and idempotent response in one PostgreSQL transaction. Concurrent identical requests must produce one tunnel and one creation event; conflicting bodies must not leave an extra tunnel. The quota check is inside the same owner lock, including requests without a key. Any failed write rolls back the create. Wire schemas and response codes are unchanged; concurrent and rollback regression tests still require a PostgreSQL run.

## 9. Admin API

**Status: who and how are decided (ADR-019, 2026-10-17). The route list and request/response shapes are still OPEN.** Read this section together with `research.md` ADR-019, which records the reasoning and — importantly — the risk the decision does *not* remove.

Intended operations: suspend/unsuspend user, terminate tunnel, list tunnels and abuse signals, drain/retire relay, rotate signing keys.

### 9.1 Authentication

- Admin is a **separate identity**, not a role on `users` (ADR-019). An admin session cannot hold tunnels; a user session carries no admin capability. **[A] — a separate identity store is a new entity; see `data-model.md` and the T-033 migration.**
- **WebAuthn (passkey) is required for the admin role. No TOTP fallback.** A single admin identity can suspend every user and rotate every signing key, so the factor is the strongest available rather than the cheapest.
- Admin sessions are **short-lived** and are **not** interchangeable with user access tokens. A user access token presented to an admin route returns `forbidden`, never a partial result.
- **Step-up re-authentication** is required immediately before a destructive action: terminate a tunnel, drain a relay, suspend a user, or rotate signing keys. A session that has not re-authenticated recently is refused for those routes.
- Recovery uses **offline recovery codes, hashed at rest**, plus a documented database-level recovery path. **[A] — mechanism unverified; T-049 must test it.** This is an explicit, narrow exception to SEC-002 ("no secrets in files"): the codes exist somewhere by necessity, at a stated location, and must never reach logs or `audit_events` (INV-5).
- PKCE for admin tooling is **`S256`-only** like every other client (ADR-018).
- Every admin action writes an `audit_events` row with `actor_type = 'admin'` (append-only). With one administrator this trail is the **primary** control, not a formality.

### 9.2 What this does not protect against — read before approving

Recorded because a security section that only lists protections is misleading:

| Protects against | Does **not** protect against |
|---|---|
| A stolen or escalated **user** session reaching admin routes | The administrator themselves, who can act on the database directly |
| An attacker holding a valid user token escalating to admin | Anyone who already has database access |
| **Accidental** destructive action, via step-up confirmation | One compromised admin account — **there is nobody else to notice** |

**Separation of duties is absent in this project and cannot be created by adding a second factor.** The single administrator is also the developer with database access. This is an **accepted risk (ADR-019)**, mitigated only by the audit trail (detective, after the fact) and step-up confirmation (accidental damage only) — neither of which stops a determined administrator. A single administrator is also a **single point of failure**: losing the only admin credential makes the service unadministrable, which is why recovery is designed rather than assumed.

### 9.3 `POST /admin/v1/auth/challenge`
- Purpose: Issue an ephemeral WebAuthn challenge for administrator passkey authentication.
- Auth: none. Request: `{username}`. Success `200`: `{challenge, rp_id, expires_in_seconds}`.
- Errors: `not_found` (unknown admin username), `validation_failed`, `rate_limited`.

### 9.4 `POST /admin/v1/auth/login`
- Purpose: Authenticate administrator using WebAuthn passkey assertion and issue short-lived admin session token (15m TTL).
- Auth: none. Request: `{username, assertion: {credential_id, authenticator_data, client_data_json, signature}}`.
- Success `200` (`Cache-Control: no-store`): `{access_token, token_type: "Bearer", expires_in}`.
- Errors: `unauthenticated` (invalid assertion, challenge mismatch/expired), `forbidden` (counter rollback / cloned authenticator, missing UP/UV flags), `not_found`.
- Checks: counter monotonicity enforced; monotonic counter updated; user presence and verification flags verified; audit event `admin.login` written.

### 9.5 `POST /admin/v1/auth/step-up`
- Purpose: Perform step-up re-authentication within an active admin session, required before destructive actions.
- Auth: admin. Request: `{assertion: {credential_id, authenticator_data, client_data_json, signature}}`.
- Success `200`: `{step_up_until, expires_in}`.
- Errors: `unauthenticated` (invalid token or assertion), `forbidden` (user token, or counter rollback).
- Effects: grants active step-up window (300 seconds TTL).

### 9.6 `POST /admin/v1/auth/recovery`
- Purpose: Redeem single-use offline recovery code to regain admin access.
- Auth: none. Request: `{username, recovery_code}`.
- Success `200` (`Cache-Control: no-store`): `{access_token, token_type: "Bearer", expires_in, recovery_warning}`.
- Errors: `unauthenticated` (invalid code), `validation_failed` (code already consumed), `not_found`, `rate_limited`.
- Checks: constant-time hash comparison; single-use consumption recorded; audit event `admin.recovery_used` written with zero recovery secrets logged (`INV-5`, `SEC-002`).

### 9.7 `POST /admin/v1/users/{id}/suspend`
- Purpose: Administrative suspension of a user account, terminating all running tunnels and revoking active sessions (`SEC-015`).
- Auth: admin. Step-up: required within 5 minutes. Request: `{reason?}`.
- Success `200`: `{user_id, status: "suspended"}`.
- Errors: `unauthenticated`, `forbidden` (user token, or step-up expired), `not_found`.
- Effects: transitions user to `suspended`, terminates all active tunnels (`INV-7`), releases ports (`INV-6`), revokes auth sessions, logs `admin.user_suspended`.

### 9.8 `POST /admin/v1/users/{id}/unsuspend`
- Purpose: Administrative reinstatement of a previously suspended user account.
- Auth: admin. Step-up: required within 5 minutes. Request: none.
- Success `200`: `{user_id, status: "active"}`.
- Errors: `unauthenticated`, `forbidden` (user token, or step-up expired), `not_found`.
- Effects: transitions user to `active`, logs `admin.user_unsuspended`.

### 9.9 `POST /admin/v1/tunnels/{id}/terminate`
- Purpose: Immediate administrative termination of a tunnel (`SEC-015`).
- Auth: admin. Step-up: required within 5 minutes. Request: `{reason?}`.
- Success `200`: `{tunnel_id, status: "terminated"}`.
- Errors: `unauthenticated`, `forbidden` (user token, or step-up expired), `not_found`.
- Effects: transitions tunnel to `stopped`, releases relay allocation and reserved ports (`INV-6`), revokes admissions, logs `tunnel.terminated`.

### 9.10 `POST /admin/v1/relays/{id}/drain`
- Purpose: Administrative command to place a relay node into draining state (`REL-002`).
- Auth: admin. Step-up: required within 5 minutes. Request: none.
- Success `200`: `{relay_id, status: "draining"}`.
- Errors: `unauthenticated`, `forbidden` (user token, or step-up expired), `not_found`.
- Effects: marks relay draining, candidate selection skips it for new allocations, relay notified on next state poll, logs `admin.relay_drained`.

### 9.11 `GET /admin/v1/audit-events`
- Purpose: Query append-only administrative and security audit events.
- Auth: admin. Query: `cursor?`, `limit?`.
- Success `200`: `{events: AuditEvent[], next_cursor?}`.
- Errors: `unauthenticated`, `forbidden` (user token).
- Data minimization: guest IPs masked (`PRIV-002`), secrets redacted (`INV-5`).

### 9.12 Open questions and spike findings

1. **WebAuthn client enforcement:** Spike completed in `docs/webauthn-admin-spike.md` confirming W3C WebAuthentication Level 3 / FIDO2 CTAP2 client enforcement, browser compatibility, platform authenticators, counter monotonicity, and disaster recovery.
2. **Owner-as-user:** Product boundary confirmed — administrator is a distinct identity store (`admins`, `admin_sessions`) separate from consumer users (`users`, `auth_sessions`).
3. **Automated administration:** Relays poll via internal API (`/internal/v1`); automated controller identities remain out of scope for single-owner operations.

## 10. Tunnel protocol (engine ↔ relay)

**Transport.** QUIC with TLS. The engine validates the relay's certificate against `relay.host`. ALPN identifier `gtt/1` is a placeholder name. The engine always connects outbound.

**Control stream.** The engine opens the first bidirectional stream. Messages are length-prefixed JSON (chosen for debuggability; control traffic is small and infrequent). The data path uses binary framing (§11).

| Message | Direction | Fields | Notes |
|---|---|---|---|
| `hello` | engine → relay | `protocol_version`, `token`, `client_version` | First message; `contracts/tunnel-hello.schema.json` |
| `hello_ack` | relay → engine | `session_id`, `heartbeat_interval_ms`, `idle_timeout_ms`, `limits`, `mappings[index, mapping_key, protocol, public_port]` | Defines mapping **indexes** used on the data path |
| `reject` | relay → engine | `code`, `retry_after_ms?` | Codes: `token_invalid`, `token_expired`, `allocation_not_found`, `version_unsupported`, `capacity`, `relay_draining`; then the relay closes |
| `ping` / `pong` | either | `seq` | Liveness above transport keep-alive and RTT sampling |
| `token_refresh` / `token_refresh_ack` | engine ↔ relay | new token | Extend a session without reconnecting |
| `flow_summary` | relay → engine | `tcp_flows`, `udp_flows`, `distinct_sources` | Drives "players seen" (traffic observed, not confirmed in-game presence) |
| `goodbye` | either | `reason` | Graceful close |

Example `hello` and `hello_ack`:
```json
{"type":"hello","protocol_version":1,"token":"<opaque-signed-token>","client_version":"0.0.0-example"}
{"type":"hello_ack","session_id":"3f2b8c1e-5a47-4d0b-9c1e-0a6f1d2e7b44","heartbeat_interval_ms":5000,"idle_timeout_ms":15000,"limits":{"max_tcp_flows":64,"max_udp_flows":64},"mappings":[{"index":0,"mapping_key":"game","protocol":"tcp","public_port":20001}]}
```
(Interval, timeout, and limit values in the example are illustrative.)

**Resumption.** On a dropped connection the engine reconnects with a valid allocation token (refreshing it via §5.9 if expired). Within the relay's grace period *(proposal)* the allocation, public ports, and UDP flow table are retained. **Established TCP connections are lost** when the tunnel connection drops, because their streams die; guests must reconnect those. UDP flows may continue if the game tolerates the gap.

## 11. Data framing

### TCP
For each guest TCP connection the relay opens a bidirectional QUIC stream to the engine and sends a header:

| Bytes | Field |
|---|---|
| 1 | frame type `0x01` (`tcp_open`) |
| 2 | mapping index (big-endian), from `hello_ack` |

The engine looks the index up in its **own local mapping list**, dials the configured local target, and replies with one status byte: `0x00` connected, `0x01` refused (unknown or disabled mapping, policy denial, or dial failure), `0x02` limit reached. After `0x00`, both directions carry raw bytes until stream FIN (half-close per direction) or reset. On status ≠ `0x00` the relay closes the guest connection.

### UDP
Each datagram is a QUIC DATAGRAM frame:

| Bytes | Field |
|---|---|
| 2 | mapping index (big-endian) |
| 4 | flow id (big-endian), assigned by the relay per (mapping, guest source address and port) |
| rest | payload |

The header is 6 bytes, so the usable payload is the QUIC maximum datagram size minus 6. Oversized packets are dropped and counted (`datagrams_dropped`). The engine creates a flow on the first datagram for a valid, enabled UDP mapping index, keeps one local UDP socket per flow so replies are routed back to the right flow id, enforces `max_udp_flows`, and expires idle flows. Datagrams with unknown mapping indexes are dropped and counted. The header carries the mapping index on every datagram because datagram and stream ordering is not guaranteed, so a separate "flow open" message could arrive late.

## 12. State machines

### Engine session

| From | Event | To |
|---|---|---|
| Idle | start with token | Connecting |
| Connecting | QUIC established | Authenticating |
| Authenticating | `hello_ack` | Active |
| Authenticating | `reject` (retryable) | Reconnecting |
| Authenticating | `reject` (`token_invalid`, `version_unsupported`) | Failed |
| Active | heartbeat loss or latency threshold | Degraded |
| Degraded | recovery | Active |
| Active/Degraded | connection lost | Reconnecting |
| Reconnecting | success | Active |
| Reconnecting | retries exhausted | Failed |
| any | user stop | Stopped |

Backoff uses exponential delay with jitter *(parameters: proposal)*. Failed and Stopped are terminal for the session; a new start is a new session. Local server reachability and players seen are **separate** indicators, not engine states.

### Allocation (relay and control plane)

`reserved` → `active` (first successful `hello`) → `released` (stop or failover) or `expired` (never used or past lifetime). `reserved` → `expired` if no `hello` arrives within a window *(proposal)*.

### Tunnel record

`created` → `active` (start) → `stopped` | `terminated` | `expired`. Terminal states are final (INV-7). This state is lifecycle intent, **not** health.

## 13. Engine ↔ UI interface

The UI calls the engine in-process (Tauri commands). Illustrative interface:

| Command | Input | Result |
|---|---|---|
| `configure_mappings` | per mapping: `mapping_key`, local host, local port | Validated or `validation_failed` list |
| `start_tunnel` | `StartTunnelResponse` | Session begins |
| `stop_tunnel` | none | Session ends |
| `probe_local` | optional mapping | L0 (reachable) and, where a probe exists, L1 result |
| `get_status` | none | Current snapshot |
| `list_provisionable` | none | Profiles that `SRV-003` allows provisioning for, and why others are excluded |
| `provision_server` | profile id | Provisioning accepted; progress via events. Refused for a non-provisionable profile |
| `start_managed_server` / `stop_managed_server` | profile id | Process started / stopped |
| `get_managed_server_status` | optional profile id | Managed server state, version, last log excerpt, world backup status |
| `backup_world` / `restore_world` | profile id, backup id | Backup created / restored |
| `update_managed_server` | profile id | Update accepted; refused without confirmation while a tunnel is active |
| `detect_tailscale` | none | Local Tailscale node status (`BackendState`, `100.64.0.0/10` IPv4, MagicDNS, share URL) (`ADR-022`) |
| `start_tailscale_share` | none | Starts Free-tier local Tailscale Node Share forwarder on `<tailscale_ip>:<port>` (`ADR-022`) |

Events pushed to the UI: `tunnel_status_changed{state, reason}`, `local_probe_result{mapping_key, level, ok, detail}`, `players_seen{tcp_flows, udp_flows}`, `engine_error{code, message}`, and (added 2026-10-17) `managed_server_status_changed{state, progress?}`, `managed_server_log{excerpt}`, `managed_server_update{state}`.

**Managed server commands are local only (2026-10-17).** These commands take no address or port from the network and none is ever returned in an API response: the local target comes from the engine's own in-process configuration (`SRV-005`, INV-1). New engine error codes are required for download failure, verification failure, license terms not accepted, disk exhaustion, and the restart limit being reached; each must have a reviewed message in the catalog built by T-085 (`UX-003`). The HTTP error model in §3 is unchanged, and **no new API routes are added** by this feature — provisioning is not a control-plane concern.

**Local target policy (proposal, needs security review).** Targets must be explicit per mapping. Loopback is allowed by default. Any non-loopback address requires explicit user confirmation per mapping. Unspecified, multicast, and link-local addresses are rejected. A deny-list of ports for commonly sensitive local services (for example SSH, RDP, SMB) is rejected unless the loaded profile explicitly allows that port. The engine will only dial targets that came from its own configuration, never from relay messages.

## 14. Versioning and compatibility

- HTTP: major version in the path (`/v1`). Within a major version, only additive changes: new optional request fields, new response fields, new routes, new error codes. Clients must tolerate unknown response fields and unknown error codes (treat as `internal`).
- Breaking changes require `/v2` with a published overlap period *(proposal)*.
- Tunnel protocol: integer `protocol_version` in `hello`; the relay supports the current and previous version *(proposal)*. Unsupported → `reject: version_unsupported`.
- Minimum client version: the server may answer `client_upgrade_required` (e.g. for a security fix).
- Mapping `position` and profile `profile_version` are stable for the life of a tunnel.

## 15. Validation rules (summary)

| Field | Rule |
|---|---|
| `name` | 1–64 characters after trimming, no control characters |
| `game_profile_id` | Must exist in the server's profile registry |
| `mapping_key` | `^[a-z][a-z0-9-]{0,31}$`, defined by the profile, unique per tunnel |
| `protocol` | `tcp` or `udp`, must match the profile's definition for that key |
| `mappings` | 1–8 entries *(bounds: proposal)* |
| invitation `expires_in_seconds` | 60–604800 *(proposal)* |
| invitation `max_claims` | 1–50 *(proposal)* |
| invitation `secret` | `^[A-Za-z0-9_-]{22,64}$` |
| Any request | Unknown fields rejected; local-target fields never accepted |

## 16. Open items

1. Identity provider and the exact `/v1/auth/exchange` shape.
2. Relay credential type (mTLS vs signed token).
3. Admin authentication model (§9).
4. Tunnel restart policy after `stop`.
5. Fallback carrier when UDP to the relay is blocked (E-14) and its protocol framing.
6. Whether guest admission by source IP is viable (E-13); if not, §5.13, §6, and the `guest_admissions` table change.
7. Signing algorithm and key rotation procedure.

## 17. Links

`api.md` (inventory), `data-model.md`, `architecture.md`, `research.md`, schemas in `contracts/`.
