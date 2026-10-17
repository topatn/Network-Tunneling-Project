# contracts.md — Component and Protocol Contracts

**Status:** DRAFT v0.1. Authoritative for API and component contracts. Route inventory: `api.md`. Entities: `data-model.md`.
**Last updated:** 2026-10-16

**Verification status.** The JSON Schemas in `docs/contracts/` are valid draft 2020-12 schemas, and the example payloads in this document (also stored in `contracts/examples.json`) were checked against them, including payloads that must be rejected. Nothing is implemented. Whether the validator enforced the `uuid` and `date-time` string formats was not independently confirmed. All numeric limits, timeouts, and lifetimes marked *(proposal)* are unvalidated starting points that `requirements.md` must fix after experiments. Items depending on guest source-IP admission carry **[E-13]**.

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
- List invitations (never returns secrets) / revoke one. Revocation also revokes its admissions. `204`. Check: owner only.

### 5.13 `GET /v1/tunnels/{id}/admissions`, `DELETE /v1/admissions/{id}` **[E-13]**
- List live guest admissions (source IPs shown to the owner only in masked form *(proposal)*) / revoke one. `204`.

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

## 9. Admin API (OPEN)

Not defined beyond intent: suspend/unsuspend user, terminate tunnel, list tunnels and abuse signals, drain/retire relay, rotate signing keys. Whether administrators are a role on `users` or a separate identity system is an unresolved security decision; admin routes must require separate, stronger authentication and produce `admin` audit events. This section must be completed before the security review.

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

`api.md` (inventory), `data-model.md`, `architecture.md`, `research.md`, schemas in `docs/contracts/`.
