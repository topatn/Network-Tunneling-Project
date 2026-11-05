//! Control-plane route handlers for `api.md` Routes 6–11 (`contracts.md` §§5.6–5.10, 8):
//!
//! * Route 6: `POST /v1/tunnels` — [`create_tunnel`] (idempotent via `Idempotency-Key`)
//! * Route 7: `GET /v1/tunnels` — [`list_tunnels`]
//! * Route 8: `GET /v1/tunnels/{id}` — [`get_tunnel`]
//! * Route 9: `POST /v1/tunnels/{id}/start` — [`start_tunnel`] (idempotent via `Idempotency-Key`)
//! * Route 10: `POST /v1/tunnels/{id}/token` — [`issue_tunnel_token`]
//! * Route 11: `POST /v1/tunnels/{id}/stop` — [`stop_tunnel`] (naturally idempotent)
//! * Failover reallocation (`REL-002`): [`reallocate_tunnel_on_failover`]
//!
//! ## Layered validation (`data-model.md` §7)
//!
//! 1. **Wire contract & `INV-1`:** raw request JSON is parsed into `serde_json::Value`,
//!    scanned by [`gtt_protocol::validate::scan_json_for_local_target`] so any host-local
//!    target key is refused before deserialization, and then deserialized into
//!    [`gtt_protocol::http::CreateTunnelRequest`] (`#[serde(deny_unknown_fields)]`) and
//!    validated with `.validate()`.
//! 2. **Authorization & entitlement (`SEC-009`):** [`crate::policy::decide`] runs before
//!    any domain mutation. Cross-owner reads return `not_found` (`404`), never
//!    `forbidden` (`403`).
//! 3. **Idempotency (`contracts.md` §8):** `Idempotency-Key` (`8..=128` ASCII graphic
//!    characters) is scoped to `(user_id, route, idem_key)` and compared
//!    against a canonical SHA-256 digest of the validated request. Replaying the same key
//!    with the same request returns the stored response; reusing the same key with a
//!    different request returns `422 idempotency_mismatch`.
//! 4. **Profile & quota domain rules (`INV-2`, `FR-004`):** the requested
//!    `game_profile_id` must exist in the loaded [`gtt_profiles::Registry`], every mapping
//!    must match the profile's declared keys and protocols, and the caller's live tunnel
//!    count must be below `max_live_tunnels`.

use chrono::{DateTime, Duration, Utc};
use sha2::{Digest as _, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use gtt_profiles::Registry;
use gtt_protocol::control::HelloAckMapping;
use gtt_protocol::http::{
    AllocatedPortDto, AllocationDto, ApiErrorCode, CreateTunnelRequest, ErrorBody, ErrorDetail,
    ErrorResponse, IssueTunnelTokenResponse, Protocol as WireProtocol, RelayEndpointDto,
    StartTunnelResponse, Tunnel as TunnelDto, TunnelMappingDto, TunnelState as WireTunnelState,
};
use gtt_protocol::token::{AllocationTokenClaims, SigningKeyRing, TokenSignError};
use gtt_protocol::validate::{is_valid_uuid, scan_json_for_local_target, ValidationError};

use crate::db::audit;
use crate::db::entities::{
    ActorType, AllocationPort, Protocol as DbProtocol, RelayAllocation, RelayNode, RelayStatus,
    Tunnel as DbTunnel, TunnelMapping,
};
use crate::db::ops::{
    append_audit_event, check_idempotency, store_idempotent_response, IdempotencyOutcome,
    IdempotentResponse, NewAuditEvent,
};
use crate::db::profile::ProfileSpec;
use crate::db::relay::{
    get_live_allocation, get_relay_node, list_allocatable_relays, list_allocation_ports,
    list_live_ports_for_relay, publish_allocation_port, relay_load, release_allocation,
    reserve_allocation,
};
use crate::db::sharing::revoke_all_for_tunnel;
use crate::db::tunnels::{
    count_live_tunnels, create_tunnel_in_transaction as db_create_tunnel,
    get_tunnel as db_get_tunnel, list_mappings, list_tunnels as db_list_tunnels,
    start_tunnel as db_start_tunnel, stop_tunnel as db_stop_tunnel, NewMapping, NewTunnel,
};
use crate::error::{PersistenceError, Rejection};
use crate::policy::{decide, Actor, Decision, ErrorCode as PolicyErrorCode, Operation, Target};

/// Canonical route identifier stored in `idempotency_keys.route` for Route 6.
pub const ROUTE_CREATE_TUNNEL: &str = "POST /v1/tunnels";

/// Canonical route identifier stored in `idempotency_keys.route` for Route 9.
pub const ROUTE_START_TUNNEL: &str = "POST /v1/tunnels/{id}/start";

/// Minimum length of an `Idempotency-Key` header value (`contracts.md` §8: 8–128 chars).
pub const MIN_IDEMPOTENCY_KEY_LEN: usize = 8;

/// Maximum length of an `Idempotency-Key` header value (`contracts.md` §8: 8–128 chars).
pub const MAX_IDEMPOTENCY_KEY_LEN: usize = 128;

/// Maximum raw request body size accepted by `POST /v1/tunnels` (16 KiB).
///
/// Checked before parsing so an oversized body cannot cause unbounded allocation
/// (`AGENTS.md` §4).
pub const MAX_REQUEST_BODY_BYTES: usize = 16 * 1024;

/// Configuration for tunnel route handlers (`contracts.md` §§5.6, 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TunnelRouteConfig {
    /// Maximum concurrent live (`created` or `active`) tunnels per user (`contracts.md` §5.6).
    pub max_live_tunnels: i64,
    /// Default lifetime of a newly created tunnel before expiry.
    pub tunnel_ttl_seconds: i64,
    /// Retention window for `idempotency_keys` rows (`contracts.md` §8).
    pub idempotency_ttl_seconds: i64,
}

impl Default for TunnelRouteConfig {
    fn default() -> Self {
        Self {
            max_live_tunnels: 5,
            tunnel_ttl_seconds: 24 * 3600,
            idempotency_ttl_seconds: 24 * 3600,
        }
    }
}

/// Configuration for relay allocation and token issuance on `POST /v1/tunnels/{id}/start`
/// and `POST /v1/tunnels/{id}/token` (`contracts.md` §§5.8, 5.9, `REL-002`, `T-043`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartTunnelConfig {
    /// Lifetime of a reserved/active relay allocation in seconds (`>= 60`).
    pub allocation_ttl_seconds: i64,
    /// Lifetime of an issued HMAC-SHA256 `allocation_token` in seconds (`60..=3600`).
    pub token_ttl_seconds: i64,
    /// Public QUIC tunnel port advertised on relay endpoints (`1..=65535`, default `4433`).
    pub relay_tunnel_port: u16,
    /// Retention window for `idempotency_keys` rows (`contracts.md` §8).
    pub idempotency_ttl_seconds: i64,
}

impl Default for StartTunnelConfig {
    fn default() -> Self {
        Self {
            allocation_ttl_seconds: 3600,
            token_ttl_seconds: 900,
            relay_tunnel_port: 4433,
            idempotency_ttl_seconds: 24 * 3600,
        }
    }
}

/// A typed HTTP-layer response holding the status code and payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiResponse<T> {
    /// HTTP status code (e.g. `200`, `201`).
    pub status: u16,
    /// True if this response was served from `idempotency_keys` replay (`contracts.md` §8).
    pub replayed: bool,
    /// Response payload.
    pub body: T,
}

/// An HTTP-layer error response pairing the HTTP status code with a schema-valid
/// [`ErrorResponse`] (`contracts.md` §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiFailure {
    /// HTTP status code paired with `response.error.code`.
    pub status: u16,
    /// Wire-format error envelope (`contracts/error-response.schema.json`).
    pub response: ErrorResponse,
}

impl ApiFailure {
    /// Construct an [`ApiFailure`] from an [`ApiErrorCode`].
    pub fn new(
        code: ApiErrorCode,
        message: &'static str,
        request_id: &str,
        details: Option<Vec<ErrorDetail>>,
    ) -> Self {
        let req_id = if request_id.trim().is_empty() {
            "req_unspecified".to_owned()
        } else {
            request_id.chars().take(128).collect()
        };
        Self {
            status: code.http_status(),
            response: ErrorResponse {
                error: ErrorBody {
                    code,
                    message: message.to_owned(),
                    request_id: req_id,
                    details,
                },
            },
        }
    }

    /// The [`ApiErrorCode`] inside this failure.
    pub fn code(&self) -> ApiErrorCode {
        self.response.error.code
    }
}

/// Validate an `Idempotency-Key` header value (`contracts.md` §8: 8–128 characters, ASCII
/// graphic, no control characters).
pub fn validate_idempotency_key(key: &str) -> Result<(), &'static str> {
    let len = key.len();
    if !(MIN_IDEMPOTENCY_KEY_LEN..=MAX_IDEMPOTENCY_KEY_LEN).contains(&len) {
        return Err("must be 8-128 characters");
    }
    if !key.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("must contain only printable ASCII characters with no whitespace");
    }
    Ok(())
}

/// Compute a canonical SHA-256 digest of a validated [`CreateTunnelRequest`].
///
/// Hashing the canonicalized request fields (rather than raw whitespace) guarantees that
/// two semantically identical JSON bodies produce the same 32-byte digest while any change
/// to `name`, `game_profile_id`, or `mappings` produces a different digest and triggers
/// `idempotency_mismatch` (`contracts.md` §8).
pub fn hash_create_tunnel_request(req: &CreateTunnelRequest) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"POST /v1/tunnels\0");
    hasher.update(req.name.as_bytes());
    hasher.update(b"\0");
    hasher.update(req.game_profile_id.as_bytes());
    hasher.update(b"\0");
    for m in &req.mappings {
        hasher.update(m.mapping_key.as_bytes());
        hasher.update(b":");
        hasher.update(m.protocol.as_str().as_bytes());
        hasher.update(if m.enabled { b":1\0" } else { b":0\0" });
    }
    hasher.finalize().into()
}

/// Parse and validate a raw JSON body for `POST /v1/tunnels` against
/// `contracts/create-tunnel-request.schema.json` and `contracts.md` §15.
pub fn parse_create_tunnel_request(
    raw_json: &str,
    request_id: &str,
) -> Result<CreateTunnelRequest, ApiFailure> {
    if raw_json.len() > MAX_REQUEST_BODY_BYTES {
        return Err(ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Request body exceeds maximum size.",
            request_id,
            Some(vec![ErrorDetail {
                field: "body".to_owned(),
                issue: "must be at most 16384 bytes".to_owned(),
            }]),
        ));
    }

    let raw_value: serde_json::Value = serde_json::from_str(raw_json).map_err(|_| {
        ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Request body is not valid JSON.",
            request_id,
            Some(vec![ErrorDetail {
                field: "body".to_owned(),
                issue: "malformed JSON".to_owned(),
            }]),
        )
    })?;

    // INV-1 check on raw keys before struct deserialization so a host-local target field is
    // reported explicitly.
    if let Err(err) = scan_json_for_local_target(&raw_value) {
        return Err(validation_error_to_failure(&err, request_id));
    }

    let req: CreateTunnelRequest = serde_json::from_value(raw_value).map_err(|_| {
        ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Request body does not match create-tunnel-request schema.",
            request_id,
            Some(vec![ErrorDetail {
                field: "body".to_owned(),
                issue: "unknown, missing, or mistyped field".to_owned(),
            }]),
        )
    })?;

    req.validate()
        .map_err(|err| validation_error_to_failure(&err, request_id))?;

    Ok(req)
}

/// Route 6 (`api.md`): `POST /v1/tunnels`.
///
/// Creates a tunnel record in state `created` with its mappings validated against the
/// profile registry (`INV-2`), enforcing account status, active-tunnel quota (`FR-004`),
/// and `Idempotency-Key` replay/conflict rules (`contracts.md` §§5.6, 8).
pub async fn create_tunnel(
    pool: &PgPool,
    profiles: &Registry,
    config: &TunnelRouteConfig,
    actor: Actor,
    idem_key: Option<&str>,
    raw_body: &str,
    request_id: &str,
) -> Result<ApiResponse<TunnelDto>, ApiFailure> {
    // 1. Authorization check (`SEC-009`, `contracts.md` §2).
    if let Decision::Deny(code) = decide(actor, Operation::CreateTunnel, Target::None) {
        return Err(policy_error_to_failure(code, request_id));
    }

    let caller_id = match actor {
        Actor::User { id, .. } => id,
        // `CreateTunnel` requires a `users` owner (`tunnels.owner_id NOT NULL REFERENCES users(id)`).
        _ => {
            return Err(ApiFailure::new(
                ApiErrorCode::Forbidden,
                "Only an authenticated user account may own a tunnel.",
                request_id,
                None,
            ));
        }
    };

    // 2. Validate `Idempotency-Key` header if supplied (`contracts.md` §8).
    if let Some(key) = idem_key {
        if let Err(issue) = validate_idempotency_key(key) {
            return Err(ApiFailure::new(
                ApiErrorCode::ValidationFailed,
                "Idempotency-Key header is invalid.",
                request_id,
                Some(vec![ErrorDetail {
                    field: "Idempotency-Key".to_owned(),
                    issue: issue.to_owned(),
                }]),
            ));
        }
    }

    // 3. Parse and validate wire body (`contracts/create-tunnel-request.schema.json`).
    let req = parse_create_tunnel_request(raw_body, request_id)?;
    let req_hash = hash_create_tunnel_request(&req);

    // Serialize creates for this owner, including requests with no idempotency key.
    // One transaction prevents a losing request or failed audit write from leaving a
    // committed tunnel behind, and keeps the quota check valid until insertion.
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| persistence_error_to_failure(&PersistenceError::from(e), request_id))?;
    sqlx::query("SET LOCAL statement_timeout = 15000")
        .execute(&mut *tx)
        .await
        .map_err(|e| persistence_error_to_failure(&PersistenceError::from(e), request_id))?;
    sqlx::query_scalar::<_, Uuid>("select id from users where id = $1 for update")
        .bind(caller_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| persistence_error_to_failure(&PersistenceError::from(e), request_id))?;

    // 4. Idempotency check before domain mutations (`contracts.md` §8).
    if let Some(key) = idem_key {
        let outcome = check_idempotency(&mut *tx, caller_id, ROUTE_CREATE_TUNNEL, key, &req_hash)
            .await
            .map_err(|e| persistence_error_to_failure(&e, request_id))?;

        match outcome {
            IdempotencyOutcome::Conflict => {
                return Err(ApiFailure::new(
                    ApiErrorCode::IdempotencyMismatch,
                    "Idempotency-Key was already used with a different request body.",
                    request_id,
                    None,
                ));
            }
            IdempotencyOutcome::Replay { status, body } => {
                let dto: TunnelDto = serde_json::from_value(body).map_err(|_| {
                    ApiFailure::new(
                        ApiErrorCode::Internal,
                        "Stored idempotent response could not be decoded.",
                        request_id,
                        None,
                    )
                })?;
                let status_u16 = u16::try_from(status).unwrap_or(201);
                return Ok(ApiResponse {
                    status: status_u16,
                    replayed: true,
                    body: dto,
                });
            }
            IdempotencyOutcome::Fresh => {}
        }
    }

    // 5. Look up profile in registry (`INV-2`, `contracts.md` §5.6).
    let Some(loaded_profile) = profiles.get(&req.game_profile_id) else {
        return Err(ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Unknown game profile.",
            request_id,
            Some(vec![ErrorDetail {
                field: "game_profile_id".to_owned(),
                issue: "profile does not exist in registry".to_owned(),
            }]),
        ));
    };
    let profile_spec = ProfileSpec::from_loaded(loaded_profile);

    // 6. Check active/live tunnel quota (`contracts.md` §5.6).
    let live_count = count_live_tunnels(&mut *tx, caller_id)
        .await
        .map_err(|e| persistence_error_to_failure(&e, request_id))?;
    if live_count >= config.max_live_tunnels {
        return Err(ApiFailure::new(
            ApiErrorCode::QuotaExceeded,
            "Account live tunnel quota reached.",
            request_id,
            None,
        ));
    }

    // 7. Create tunnel and mappings in PostgreSQL (`INV-2` enforced inside transaction).
    let new_mappings: Vec<NewMapping> = req
        .mappings
        .iter()
        .map(|m| NewMapping {
            mapping_key: m.mapping_key.clone(),
            protocol: match m.protocol {
                WireProtocol::Tcp => DbProtocol::Tcp,
                WireProtocol::Udp => DbProtocol::Udp,
            },
            enabled: m.enabled,
        })
        .collect();

    let now = Utc::now();
    let expires_at = now + Duration::seconds(config.tunnel_ttl_seconds.max(60));
    let new_tunnel = NewTunnel {
        owner_id: caller_id,
        name: req.name.trim(),
        game_profile_id: &req.game_profile_id,
        profile_version: profile_spec.version,
        mappings: &new_mappings,
        expires_at,
    };

    let created = db_create_tunnel(&mut tx, &new_tunnel, &profile_spec)
        .await
        .map_err(|e| persistence_error_to_failure(&e, request_id))?;

    let db_mappings = list_mappings(&mut *tx, caller_id, created.id)
        .await
        .map_err(|e| persistence_error_to_failure(&e, request_id))?;

    let dto = assemble_tunnel_dto(&created, &db_mappings, request_id)?;

    let metadata = audit::build([
        (
            "profile_id".to_owned(),
            serde_json::Value::String(created.game_profile_id.clone()),
        ),
        (
            "state_after".to_owned(),
            serde_json::Value::String("created".to_owned()),
        ),
    ])
    .map_err(|e| persistence_error_to_failure(&e, request_id))?;

    append_audit_event(
        &mut *tx,
        &NewAuditEvent {
            actor_type: ActorType::User,
            actor_id: Some(caller_id),
            action: "tunnel.created",
            subject_type: "tunnel",
            subject_id: Some(created.id),
            request_id: Some(request_id),
            metadata: &metadata,
        },
    )
    .await
    .map_err(|e| persistence_error_to_failure(&e, request_id))?;

    // 8. Store idempotency record if `Idempotency-Key` was supplied (`contracts.md` §8).
    if let Some(key) = idem_key {
        let body_value = serde_json::to_value(&dto).map_err(|_| {
            ApiFailure::new(
                ApiErrorCode::Internal,
                "Failed to serialize tunnel response.",
                request_id,
                None,
            )
        })?;
        let idem_expires = now + Duration::seconds(config.idempotency_ttl_seconds.max(60));
        let stored = store_idempotent_response(
            &mut *tx,
            &IdempotentResponse {
                user_id: caller_id,
                route: ROUTE_CREATE_TUNNEL,
                idem_key: key,
                request_hash: &req_hash,
                status: 201,
                body: &body_value,
                expires_at: idem_expires,
            },
        )
        .await
        .map_err(|e| persistence_error_to_failure(&e, request_id))?;

        // Defensive against writers outside this route's owner lock: never commit
        // a second tunnel if the key could not be stored.
        if !stored {
            return Err(ApiFailure::new(
                ApiErrorCode::Internal,
                "Could not atomically store tunnel response.",
                request_id,
                None,
            ));
        }
    }

    tx.commit()
        .await
        .map_err(|e| persistence_error_to_failure(&PersistenceError::from(e), request_id))?;

    Ok(ApiResponse {
        status: 201,
        replayed: false,
        body: dto,
    })
}

/// Route 7 (`api.md`): `GET /v1/tunnels`.
///
/// Lists the authenticated user's own tunnels, newest first (`contracts.md` §5.7).
pub async fn list_tunnels(
    pool: &PgPool,
    actor: Actor,
    limit: i64,
    request_id: &str,
) -> Result<ApiResponse<Vec<TunnelDto>>, ApiFailure> {
    if let Decision::Deny(code) = decide(actor, Operation::ListTunnels, Target::None) {
        return Err(policy_error_to_failure(code, request_id));
    }

    let caller_id = match actor {
        Actor::User { id, .. } => id,
        _ => {
            return Err(ApiFailure::new(
                ApiErrorCode::Forbidden,
                "Only an authenticated user account may list own tunnels.",
                request_id,
                None,
            ));
        }
    };

    let rows = db_list_tunnels(pool, caller_id, limit)
        .await
        .map_err(|e| persistence_error_to_failure(&e, request_id))?;

    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let mappings = list_mappings(pool, caller_id, row.id)
            .await
            .map_err(|e| persistence_error_to_failure(&e, request_id))?;
        out.push(assemble_tunnel_dto(row, &mappings, request_id)?);
    }

    Ok(ApiResponse {
        status: 200,
        replayed: false,
        body: out,
    })
}

/// Route 8 (`api.md`): `GET /v1/tunnels/{id}`.
///
/// Fetches a single tunnel scoped to its owner (`contracts.md` §5.7). Cross-owner access
/// and absent tunnels both resolve to `Target::UserOwned { owner: None }` and return
/// `404 not_found` (`SEC-009`).
pub async fn get_tunnel(
    pool: &PgPool,
    actor: Actor,
    tunnel_id_str: &str,
    request_id: &str,
) -> Result<ApiResponse<TunnelDto>, ApiFailure> {
    // Check unauthenticated / capability before inspecting path parameters so anonymous
    // callers always receive `401 unauthenticated` (`policy.rs` step 1).
    if matches!(actor, Actor::Anonymous | Actor::Guest { .. }) {
        return Err(ApiFailure::new(
            ApiErrorCode::Unauthenticated,
            "Authentication required.",
            request_id,
            None,
        ));
    }

    if !is_valid_uuid(tunnel_id_str) {
        return Err(ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Tunnel id is not a valid UUID.",
            request_id,
            Some(vec![ErrorDetail {
                field: "id".to_owned(),
                issue: "must be a valid UUID".to_owned(),
            }]),
        ));
    }
    let Ok(tunnel_id) = Uuid::parse_str(tunnel_id_str) else {
        return Err(ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Tunnel id is not a valid UUID.",
            request_id,
            None,
        ));
    };

    let caller_id = match actor {
        Actor::User { id, .. } => id,
        other => {
            // Relay or Admin hitting the user-scoped getter with no visible user row.
            let decision = decide(
                other,
                Operation::ViewTunnel,
                Target::UserOwned { owner: None },
            );
            let code = decision.error().unwrap_or(PolicyErrorCode::NotFound);
            return Err(policy_error_to_failure(code, request_id));
        }
    };

    let maybe_tunnel = db_get_tunnel(pool, caller_id, tunnel_id)
        .await
        .map_err(|e| persistence_error_to_failure(&e, request_id))?;

    let owner_ref = maybe_tunnel.as_ref().map(|t| &t.owner_id);
    if let Decision::Deny(code) = decide(
        actor,
        Operation::ViewTunnel,
        Target::UserOwned { owner: owner_ref },
    ) {
        return Err(policy_error_to_failure(code, request_id));
    }

    let Some(tunnel) = maybe_tunnel else {
        return Err(ApiFailure::new(
            ApiErrorCode::NotFound,
            "Tunnel not found.",
            request_id,
            None,
        ));
    };

    let mappings = list_mappings(pool, caller_id, tunnel.id)
        .await
        .map_err(|e| persistence_error_to_failure(&e, request_id))?;

    let dto = assemble_tunnel_dto(&tunnel, &mappings, request_id)?;
    Ok(ApiResponse {
        status: 200,
        replayed: false,
        body: dto,
    })
}

/// Compute a canonical SHA-256 digest of a `POST /v1/tunnels/{id}/start` request (`contracts.md` §8).
///
/// Binds the idempotency record to the specific `tunnel_id` so reusing an `Idempotency-Key`
/// across two different tunnels is rejected with `422 idempotency_mismatch`.
pub fn hash_start_tunnel_request(tunnel_id: Uuid) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"POST /v1/tunnels/{id}/start\0");
    hasher.update(tunnel_id.as_bytes());
    hasher.finalize().into()
}

/// Select healthy, non-excluded relays with remaining capacity, ordered by lowest live load
/// (`REL-002`, `INV-6`).
///
/// Pure function so relay filtering, capacity enforcement, and failover exclusion can be
/// unit-tested without a database connection.
pub fn select_candidate_relays(
    relays: &[RelayNode],
    loads: &[(Uuid, i64, i32)],
    excluded_relay_id: Option<Uuid>,
) -> Vec<RelayNode> {
    let mut scored: Vec<(i64, RelayNode)> = Vec::new();
    for relay in relays {
        let Some(status) = RelayStatus::parse(&relay.status) else {
            continue;
        };
        if !status.accepts_new_allocations() {
            continue;
        }
        if excluded_relay_id == Some(relay.id) {
            continue;
        }
        if relay.max_allocations <= 0 {
            continue;
        }
        let live_load = loads
            .iter()
            .find(|(id, _, _)| *id == relay.id)
            .map(|(_, live, _)| *live)
            .unwrap_or(0);
        if live_load >= i64::from(relay.max_allocations) {
            continue;
        }
        scored.push((live_load, relay.clone()));
    }

    scored.sort_by(|(load_a, r_a), (load_b, r_b)| {
        load_a
            .cmp(load_b)
            .then_with(|| r_a.name.cmp(&r_b.name))
            .then_with(|| r_a.id.cmp(&r_b.id))
    });

    scored.into_iter().map(|(_, r)| r).collect()
}

/// Planned port assignment for one enabled [`TunnelMapping`] (`INV-4`, `INV-6`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedPortAssignment {
    /// Mapping row UUID.
    pub mapping_id: Uuid,
    /// Profile mapping key.
    pub mapping_key: String,
    /// Wire index (`0..=7`).
    pub position: u8,
    /// Transport protocol.
    pub protocol: DbProtocol,
    /// Reserved public port on the relay (`1024..=65535`).
    pub public_port: u16,
}

/// Assign free public ports within `[port_range_start, port_range_end]` for all enabled
/// mappings of a tunnel, avoiding any live ports on the relay (`INV-6`).
///
/// Prefers distinct public port numbers across mappings in the same allocation when the
/// relay's port range has enough distinct ports, and falls back to per-`(protocol, port)`
/// uniqueness when a narrow port range is shared by TCP and UDP mappings.
pub fn assign_free_ports(
    port_range_start: i32,
    port_range_end: i32,
    live_ports: &[AllocationPort],
    mappings: &[TunnelMapping],
) -> Option<Vec<PlannedPortAssignment>> {
    if port_range_start < 1024 || port_range_end > 65535 || port_range_start > port_range_end {
        return None;
    }

    let mut enabled: Vec<&TunnelMapping> = mappings.iter().filter(|m| m.enabled).collect();
    if enabled.is_empty() {
        return None;
    }
    enabled.sort_by_key(|m| m.position);

    let mut occupied: Vec<(DbProtocol, i32)> = Vec::with_capacity(live_ports.len());
    for lp in live_ports {
        if lp.released_at.is_none() {
            if let Some(proto) = DbProtocol::parse(&lp.protocol) {
                occupied.push((proto, lp.public_port));
            }
        }
    }

    let mut assigned_ports_in_batch: Vec<i32> = Vec::with_capacity(enabled.len());
    let mut plan = Vec::with_capacity(enabled.len());

    for mapping in enabled {
        let proto = DbProtocol::parse(&mapping.protocol)?;
        let position = u8::try_from(mapping.position).ok()?;

        // First pass: prefer a port that is free for `proto` and not yet used by any live port
        // or earlier mapping in this tunnel's batch.
        let mut chosen = (port_range_start..=port_range_end).find(|candidate_port| {
            !occupied.iter().any(|(_, p)| *p == *candidate_port)
                && !assigned_ports_in_batch.contains(candidate_port)
        });

        // Second pass: if all port numbers have at least one protocol bound, allow sharing a
        // port number across different protocols (`allocation_ports_live_port_idx` is on
        // `(relay_id, protocol, public_port)`).
        if chosen.is_none() {
            chosen = (port_range_start..=port_range_end).find(|candidate_port| {
                !occupied
                    .iter()
                    .any(|(pr, p)| *pr == proto && *p == *candidate_port)
            });
        }

        let port_i32 = chosen?;
        let port_u16 = u16::try_from(port_i32).ok()?;
        occupied.push((proto, port_i32));
        assigned_ports_in_batch.push(port_i32);

        plan.push(PlannedPortAssignment {
            mapping_id: mapping.id,
            mapping_key: mapping.mapping_key.clone(),
            position,
            protocol: proto,
            public_port: port_u16,
        });
    }

    Some(plan)
}

/// Input parameters for [`build_allocation_token_claims`] (`contracts.md` §2).
#[derive(Debug, Clone, Copy)]
pub struct TokenClaimsInput<'a> {
    /// Token issuer (`iss`).
    pub issuer: &'a str,
    /// Target relay UUID (`aud`).
    pub relay_id: Uuid,
    /// Tunnel UUID (`sub`).
    pub tunnel_id: Uuid,
    /// Allocation UUID (`aid`).
    pub allocation_id: Uuid,
    /// Tunnel mappings (`tunnel_mappings`).
    pub mappings: &'a [TunnelMapping],
    /// Live published ports (`allocation_ports`).
    pub ports: &'a [AllocationPort],
    /// Issued-at timestamp (`iat`).
    pub now: DateTime<Utc>,
    /// Expiration timestamp (`exp`).
    pub exp: DateTime<Utc>,
    /// Unique token identifier (`jti`).
    pub jti: Uuid,
}

/// Build validated [`AllocationTokenClaims`] for a tunnel allocation (`contracts.md` §2).
pub fn build_allocation_token_claims(
    input: &TokenClaimsInput<'_>,
) -> Result<AllocationTokenClaims, ApiFailure> {
    let mut ack_mappings = Vec::new();
    for m in input.mappings.iter().filter(|m| m.enabled) {
        let Some(port_row) = input
            .ports
            .iter()
            .find(|p| p.mapping_id == m.id && p.released_at.is_none())
        else {
            return Err(ApiFailure::new(
                ApiErrorCode::Internal,
                "Enabled mapping is missing a live allocation port.",
                "req_token_claims",
                None,
            ));
        };
        let protocol = match m.protocol.as_str() {
            "tcp" => WireProtocol::Tcp,
            "udp" => WireProtocol::Udp,
            _ => {
                return Err(ApiFailure::new(
                    ApiErrorCode::Internal,
                    "Stored mapping protocol is invalid.",
                    "req_token_claims",
                    None,
                ));
            }
        };
        let index = u16::try_from(m.position).map_err(|_| {
            ApiFailure::new(
                ApiErrorCode::Internal,
                "Stored mapping position is out of bounds.",
                "req_token_claims",
                None,
            )
        })?;
        let public_port = u16::try_from(port_row.public_port).map_err(|_| {
            ApiFailure::new(
                ApiErrorCode::Internal,
                "Stored public port is out of bounds.",
                "req_token_claims",
                None,
            )
        })?;
        ack_mappings.push(HelloAckMapping {
            index,
            mapping_key: m.mapping_key.clone(),
            protocol,
            public_port,
        });
    }

    ack_mappings.sort_by_key(|m| m.index);

    let iat_s = input.now.timestamp().max(1);
    let exp_s = input.exp.timestamp().max(iat_s.saturating_add(1));

    let claims = AllocationTokenClaims {
        iss: input.issuer.to_owned(),
        aud: input.relay_id.to_string(),
        sub: input.tunnel_id.to_string(),
        aid: input.allocation_id.to_string(),
        mappings: ack_mappings,
        iat: iat_s,
        exp: exp_s,
        jti: input.jti.to_string(),
    };

    claims.validate().map_err(|_| {
        ApiFailure::new(
            ApiErrorCode::Internal,
            "Constructed allocation token claims failed validation.",
            "req_token_claims",
            None,
        )
    })?;

    Ok(claims)
}

/// Route 9 (`api.md`): `POST /v1/tunnels/{id}/start`.
///
/// Picks a healthy relay with capacity (`REL-002`), reserves public ports for enabled
/// mappings (`INV-3`, `INV-4`, `INV-6`), transitions `tunnels.state -> active`, signs an
/// `allocation_token`, appends an `audit_events` row (`INV-5`), and returns a
/// schema-validated [`StartTunnelResponse`] (`contracts.md` §5.8).
pub async fn start_tunnel(
    pool: &PgPool,
    key_ring: &SigningKeyRing,
    config: &StartTunnelConfig,
    actor: Actor,
    tunnel_id_str: &str,
    idem_key: Option<&str>,
    request_id: &str,
) -> Result<ApiResponse<StartTunnelResponse>, ApiFailure> {
    let (caller_id, tunnel) = authorize_tunnel_operation(
        pool,
        actor,
        Operation::StartTunnel,
        tunnel_id_str,
        request_id,
    )
    .await?;

    if let Some(key) = idem_key {
        if let Err(issue) = validate_idempotency_key(key) {
            return Err(ApiFailure::new(
                ApiErrorCode::ValidationFailed,
                "Idempotency-Key header is invalid.",
                request_id,
                Some(vec![ErrorDetail {
                    field: "Idempotency-Key".to_owned(),
                    issue: issue.to_owned(),
                }]),
            ));
        }
    }

    let req_hash = hash_start_tunnel_request(tunnel.id);
    if let Some(key) = idem_key {
        let outcome = check_idempotency(pool, caller_id, ROUTE_START_TUNNEL, key, &req_hash)
            .await
            .map_err(|e| persistence_error_to_failure(&e, request_id))?;

        match outcome {
            IdempotencyOutcome::Conflict => {
                return Err(ApiFailure::new(
                    ApiErrorCode::IdempotencyMismatch,
                    "Idempotency-Key was already used for a different tunnel start request.",
                    request_id,
                    None,
                ));
            }
            IdempotencyOutcome::Replay { status, body } => {
                let dto: StartTunnelResponse = serde_json::from_value(body).map_err(|_| {
                    ApiFailure::new(
                        ApiErrorCode::Internal,
                        "Stored idempotent start response could not be decoded.",
                        request_id,
                        None,
                    )
                })?;
                return Ok(ApiResponse {
                    status: u16::try_from(status).unwrap_or(200),
                    replayed: true,
                    body: dto,
                });
            }
            IdempotencyOutcome::Fresh => {}
        }
    }

    // Terminal states cannot be started (`contracts.md` §5.8, `INV-7`).
    if matches!(tunnel.state.as_str(), "stopped" | "terminated" | "expired") {
        return Err(ApiFailure::new(
            ApiErrorCode::Conflict,
            "Tunnel is in a terminal state and cannot be started.",
            request_id,
            None,
        ));
    }

    let mappings = list_mappings(pool, caller_id, tunnel.id)
        .await
        .map_err(|e| persistence_error_to_failure(&e, request_id))?;
    if !mappings.iter().any(|m| m.enabled) {
        return Err(ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Tunnel must have at least one enabled mapping to start.",
            request_id,
            None,
        ));
    }

    let now = Utc::now();

    // `contracts.md` §5.8: "a repeat returns the same allocation while it is live."
    if let Some(live_alloc) = get_live_allocation(pool, tunnel.id)
        .await
        .map_err(|e| persistence_error_to_failure(&e, request_id))?
    {
        if live_alloc.expires_at > now {
            if let Some(relay_node) = get_relay_node(pool, live_alloc.relay_id)
                .await
                .map_err(|e| persistence_error_to_failure(&e, request_id))?
            {
                if RelayStatus::parse(&relay_node.status) == Some(RelayStatus::Healthy) {
                    let ports = list_allocation_ports(pool, live_alloc.id)
                        .await
                        .map_err(|e| persistence_error_to_failure(&e, request_id))?;
                    let dto = assemble_start_tunnel_response(
                        key_ring,
                        config,
                        &tunnel,
                        &relay_node,
                        &live_alloc,
                        &mappings,
                        &ports,
                        now,
                        request_id,
                    )?;
                    store_start_idempotency_if_present(
                        pool,
                        caller_id,
                        idem_key,
                        &req_hash,
                        &dto,
                        now,
                        config.idempotency_ttl_seconds,
                        request_id,
                    )
                    .await?;
                    return Ok(ApiResponse {
                        status: 200,
                        replayed: true,
                        body: dto,
                    });
                }
            }
        }
        // If an existing live allocation is expired or on an unhealthy relay, release it before
        // allocating a healthy replacement.
        release_allocation(pool, live_alloc.id)
            .await
            .map_err(|e| persistence_error_to_failure(&e, request_id))?;
    }

    let (relay_node, allocation, ports) =
        allocate_relay_for_tunnel(pool, tunnel.id, &mappings, config, None, now, request_id)
            .await?;

    if tunnel.state == "created" {
        let transitioned = db_start_tunnel(pool, caller_id, tunnel.id)
            .await
            .map_err(|e| persistence_error_to_failure(&e, request_id))?;
        if !transitioned {
            let _ = release_allocation(pool, allocation.id).await;
            return Err(ApiFailure::new(
                ApiErrorCode::Conflict,
                "Tunnel state changed concurrently and could not transition to active.",
                request_id,
                None,
            ));
        }
    }

    let dto = assemble_start_tunnel_response(
        key_ring,
        config,
        &tunnel,
        &relay_node,
        &allocation,
        &mappings,
        &ports,
        now,
        request_id,
    )?;

    let port_count = i64::try_from(ports.len()).unwrap_or(0);
    let metadata = audit::build([
        (
            "state_before".to_owned(),
            serde_json::Value::String(tunnel.state.clone()),
        ),
        (
            "state_after".to_owned(),
            serde_json::Value::String("active".to_owned()),
        ),
        ("count".to_owned(), serde_json::Value::from(port_count)),
    ])
    .map_err(|e| persistence_error_to_failure(&e, request_id))?;

    append_audit_event(
        pool,
        &NewAuditEvent {
            actor_type: ActorType::User,
            actor_id: Some(caller_id),
            action: "tunnel.started",
            subject_type: "tunnel",
            subject_id: Some(tunnel.id),
            request_id: Some(request_id),
            metadata: &metadata,
        },
    )
    .await
    .map_err(|e| persistence_error_to_failure(&e, request_id))?;

    store_start_idempotency_if_present(
        pool,
        caller_id,
        idem_key,
        &req_hash,
        &dto,
        now,
        config.idempotency_ttl_seconds,
        request_id,
    )
    .await?;

    Ok(ApiResponse {
        status: 200,
        replayed: false,
        body: dto,
    })
}

/// Route 10 (`api.md`): `POST /v1/tunnels/{id}/token`.
///
/// Issues a fresh signed `allocation_token` for the tunnel's current live allocation
/// (`contracts.md` §5.9). Returns `409 conflict` when the tunnel is not `active` or has
/// no live allocation.
pub async fn issue_tunnel_token(
    pool: &PgPool,
    key_ring: &SigningKeyRing,
    config: &StartTunnelConfig,
    actor: Actor,
    tunnel_id_str: &str,
    request_id: &str,
) -> Result<ApiResponse<IssueTunnelTokenResponse>, ApiFailure> {
    let (caller_id, tunnel) = authorize_tunnel_operation(
        pool,
        actor,
        Operation::StartTunnel,
        tunnel_id_str,
        request_id,
    )
    .await?;

    if tunnel.state != "active" {
        return Err(ApiFailure::new(
            ApiErrorCode::Conflict,
            "Tunnel is not active.",
            request_id,
            None,
        ));
    }

    let now = Utc::now();
    let Some(live_alloc) = get_live_allocation(pool, tunnel.id)
        .await
        .map_err(|e| persistence_error_to_failure(&e, request_id))?
    else {
        return Err(ApiFailure::new(
            ApiErrorCode::Conflict,
            "Tunnel has no live relay allocation.",
            request_id,
            None,
        ));
    };

    if live_alloc.expires_at <= now {
        return Err(ApiFailure::new(
            ApiErrorCode::Conflict,
            "Tunnel relay allocation has expired.",
            request_id,
            None,
        ));
    }

    let mappings = list_mappings(pool, caller_id, tunnel.id)
        .await
        .map_err(|e| persistence_error_to_failure(&e, request_id))?;
    let ports = list_allocation_ports(pool, live_alloc.id)
        .await
        .map_err(|e| persistence_error_to_failure(&e, request_id))?;

    let token_ttl = Duration::seconds(config.token_ttl_seconds.clamp(60, 3600));
    let token_exp = (now + token_ttl).min(live_alloc.expires_at);
    let claims = build_allocation_token_claims(&TokenClaimsInput {
        issuer: key_ring.issuer(),
        relay_id: live_alloc.relay_id,
        tunnel_id: tunnel.id,
        allocation_id: live_alloc.id,
        mappings: &mappings,
        ports: &ports,
        now,
        exp: token_exp,
        jti: Uuid::new_v4(),
    })?;

    let allocation_token = key_ring
        .sign_allocation_token(&claims)
        .map_err(|e| token_sign_error_to_failure(&e, request_id))?;

    let body = IssueTunnelTokenResponse {
        allocation_token,
        expires_at: format_rfc3339_utc(token_exp),
    };
    body.validate().map_err(|_| {
        ApiFailure::new(
            ApiErrorCode::Internal,
            "Assembled token response failed validation.",
            request_id,
            None,
        )
    })?;

    Ok(ApiResponse {
        status: 200,
        replayed: false,
        body,
    })
}

/// Route 11 (`api.md`): `POST /v1/tunnels/{id}/stop`.
///
/// Naturally idempotent (`contracts.md` §§5.10, 8): transitions `tunnels.state -> stopped`,
/// releases any live `relay_allocations` and `allocation_ports`, revokes all `invitations`
/// and `guest_admissions` for the tunnel, writes an `audit_events` row (`INV-5`), and
/// returns the updated [`TunnelDto`].
pub async fn stop_tunnel(
    pool: &PgPool,
    actor: Actor,
    tunnel_id_str: &str,
    request_id: &str,
) -> Result<ApiResponse<TunnelDto>, ApiFailure> {
    let (caller_id, tunnel) = authorize_tunnel_operation(
        pool,
        actor,
        Operation::StopTunnel,
        tunnel_id_str,
        request_id,
    )
    .await?;

    let mappings = list_mappings(pool, caller_id, tunnel.id)
        .await
        .map_err(|e| persistence_error_to_failure(&e, request_id))?;

    // Natural idempotency: already `stopped` returns 200 with the current tunnel representation.
    if tunnel.state == "stopped" {
        let dto = assemble_tunnel_dto(&tunnel, &mappings, request_id)?;
        return Ok(ApiResponse {
            status: 200,
            replayed: true,
            body: dto,
        });
    }

    // Other terminal states (`terminated`, `expired`) are final (`INV-7`) and cannot transition
    // to `stopped`.
    if matches!(tunnel.state.as_str(), "terminated" | "expired") {
        return Err(ApiFailure::new(
            ApiErrorCode::Conflict,
            "Tunnel is already in a terminal state.",
            request_id,
            None,
        ));
    }

    // 1. Release live relay allocation and free its public ports (`INV-6`).
    if let Some(live_alloc) = get_live_allocation(pool, tunnel.id)
        .await
        .map_err(|e| persistence_error_to_failure(&e, request_id))?
    {
        release_allocation(pool, live_alloc.id)
            .await
            .map_err(|e| persistence_error_to_failure(&e, request_id))?;
    }

    // 2. Revoke all active invitations and guest admissions for the tunnel (`contracts.md` §5.10).
    revoke_all_for_tunnel(pool, caller_id, tunnel.id)
        .await
        .map_err(|e| persistence_error_to_failure(&e, request_id))?;

    // 3. Transition tunnel record to `stopped` (`INV-7`).
    db_stop_tunnel(pool, caller_id, tunnel.id)
        .await
        .map_err(|e| persistence_error_to_failure(&e, request_id))?;

    let updated = db_get_tunnel(pool, caller_id, tunnel.id)
        .await
        .map_err(|e| persistence_error_to_failure(&e, request_id))?
        .ok_or_else(|| {
            ApiFailure::new(
                ApiErrorCode::Internal,
                "Stopped tunnel could not be re-read.",
                request_id,
                None,
            )
        })?;

    // 4. Append audit event (`INV-5`).
    let metadata = audit::build([
        (
            "state_before".to_owned(),
            serde_json::Value::String(tunnel.state),
        ),
        (
            "state_after".to_owned(),
            serde_json::Value::String("stopped".to_owned()),
        ),
        (
            "reason".to_owned(),
            serde_json::Value::String("owner_requested".to_owned()),
        ),
    ])
    .map_err(|e| persistence_error_to_failure(&e, request_id))?;

    append_audit_event(
        pool,
        &NewAuditEvent {
            actor_type: ActorType::User,
            actor_id: Some(caller_id),
            action: "tunnel.stopped",
            subject_type: "tunnel",
            subject_id: Some(updated.id),
            request_id: Some(request_id),
            metadata: &metadata,
        },
    )
    .await
    .map_err(|e| persistence_error_to_failure(&e, request_id))?;

    let dto = assemble_tunnel_dto(&updated, &mappings, request_id)?;
    Ok(ApiResponse {
        status: 200,
        replayed: false,
        body: dto,
    })
}

/// Failover reallocation procedure (`REL-002`, `T-043`).
///
/// Releases the tunnel's current allocation on a failed or draining relay, selects a
/// remaining healthy relay with available capacity (explicitly excluding the previous relay),
/// reserves new public ports, mints a fresh `allocation_token`, and appends an `audit_events`
/// row.
pub async fn reallocate_tunnel_on_failover(
    pool: &PgPool,
    key_ring: &SigningKeyRing,
    config: &StartTunnelConfig,
    actor: Actor,
    tunnel_id_str: &str,
    request_id: &str,
) -> Result<ApiResponse<StartTunnelResponse>, ApiFailure> {
    let (caller_id, tunnel) = authorize_tunnel_operation(
        pool,
        actor,
        Operation::StartTunnel,
        tunnel_id_str,
        request_id,
    )
    .await?;

    if tunnel.state != "active" {
        return Err(ApiFailure::new(
            ApiErrorCode::Conflict,
            "Only an active tunnel can be reallocated on relay failover.",
            request_id,
            None,
        ));
    }

    let mappings = list_mappings(pool, caller_id, tunnel.id)
        .await
        .map_err(|e| persistence_error_to_failure(&e, request_id))?;

    let previous_relay_id = if let Some(live_alloc) = get_live_allocation(pool, tunnel.id)
        .await
        .map_err(|e| persistence_error_to_failure(&e, request_id))?
    {
        let prev_relay = live_alloc.relay_id;
        release_allocation(pool, live_alloc.id)
            .await
            .map_err(|e| persistence_error_to_failure(&e, request_id))?;
        Some(prev_relay)
    } else {
        None
    };

    let now = Utc::now();
    let (relay_node, allocation, ports) = allocate_relay_for_tunnel(
        pool,
        tunnel.id,
        &mappings,
        config,
        previous_relay_id,
        now,
        request_id,
    )
    .await?;

    let dto = assemble_start_tunnel_response(
        key_ring,
        config,
        &tunnel,
        &relay_node,
        &allocation,
        &mappings,
        &ports,
        now,
        request_id,
    )?;

    let metadata = audit::build([
        (
            "state_before".to_owned(),
            serde_json::Value::String("active".to_owned()),
        ),
        (
            "state_after".to_owned(),
            serde_json::Value::String("active".to_owned()),
        ),
        (
            "reason".to_owned(),
            serde_json::Value::String("relay_failover".to_owned()),
        ),
    ])
    .map_err(|e| persistence_error_to_failure(&e, request_id))?;

    append_audit_event(
        pool,
        &NewAuditEvent {
            actor_type: ActorType::User,
            actor_id: Some(caller_id),
            action: "tunnel.reallocated",
            subject_type: "tunnel",
            subject_id: Some(tunnel.id),
            request_id: Some(request_id),
            metadata: &metadata,
        },
    )
    .await
    .map_err(|e| persistence_error_to_failure(&e, request_id))?;

    Ok(ApiResponse {
        status: 200,
        replayed: false,
        body: dto,
    })
}

async fn authorize_tunnel_operation(
    pool: &PgPool,
    actor: Actor,
    operation: Operation,
    tunnel_id_str: &str,
    request_id: &str,
) -> Result<(Uuid, DbTunnel), ApiFailure> {
    if matches!(actor, Actor::Anonymous | Actor::Guest { .. }) {
        return Err(ApiFailure::new(
            ApiErrorCode::Unauthenticated,
            "Authentication required.",
            request_id,
            None,
        ));
    }

    if !is_valid_uuid(tunnel_id_str) {
        return Err(ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Tunnel id is not a valid UUID.",
            request_id,
            Some(vec![ErrorDetail {
                field: "id".to_owned(),
                issue: "must be a valid UUID".to_owned(),
            }]),
        ));
    }
    let Ok(tunnel_id) = Uuid::parse_str(tunnel_id_str) else {
        return Err(ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Tunnel id is not a valid UUID.",
            request_id,
            None,
        ));
    };

    let caller_id = match actor {
        Actor::User { id, .. } => id,
        other => {
            let decision = decide(other, operation, Target::UserOwned { owner: None });
            let code = decision.error().unwrap_or(PolicyErrorCode::NotFound);
            return Err(policy_error_to_failure(code, request_id));
        }
    };

    let maybe_tunnel = db_get_tunnel(pool, caller_id, tunnel_id)
        .await
        .map_err(|e| persistence_error_to_failure(&e, request_id))?;

    let owner_ref = maybe_tunnel.as_ref().map(|t| &t.owner_id);
    if let Decision::Deny(code) = decide(actor, operation, Target::UserOwned { owner: owner_ref }) {
        return Err(policy_error_to_failure(code, request_id));
    }

    let Some(tunnel) = maybe_tunnel else {
        return Err(ApiFailure::new(
            ApiErrorCode::NotFound,
            "Tunnel not found.",
            request_id,
            None,
        ));
    };

    Ok((caller_id, tunnel))
}

async fn allocate_relay_for_tunnel(
    pool: &PgPool,
    tunnel_id: Uuid,
    mappings: &[TunnelMapping],
    config: &StartTunnelConfig,
    excluded_relay_id: Option<Uuid>,
    now: DateTime<Utc>,
    request_id: &str,
) -> Result<(RelayNode, RelayAllocation, Vec<AllocationPort>), ApiFailure> {
    let healthy_relays = list_allocatable_relays(pool, 200)
        .await
        .map_err(|e| persistence_error_to_failure(&e, request_id))?;
    let loads = relay_load(pool)
        .await
        .map_err(|e| persistence_error_to_failure(&e, request_id))?;

    let candidates = select_candidate_relays(&healthy_relays, &loads, excluded_relay_id);
    if candidates.is_empty() {
        return Err(ApiFailure::new(
            ApiErrorCode::RelayUnavailable,
            "No healthy relay with available capacity.",
            request_id,
            None,
        ));
    }

    let alloc_ttl = Duration::seconds(config.allocation_ttl_seconds.max(60));
    let alloc_expires_at = now + alloc_ttl;

    for candidate in candidates {
        let live_ports = list_live_ports_for_relay(pool, candidate.id)
            .await
            .map_err(|e| persistence_error_to_failure(&e, request_id))?;

        let Some(planned_ports) = assign_free_ports(
            candidate.port_range_start,
            candidate.port_range_end,
            &live_ports,
            mappings,
        ) else {
            continue;
        };

        let allocation =
            match reserve_allocation(pool, tunnel_id, candidate.id, alloc_expires_at).await {
                Ok(alloc) => alloc,
                Err(PersistenceError::Rejected(_)) => {
                    return Err(ApiFailure::new(
                        ApiErrorCode::Conflict,
                        "Tunnel already has a live relay allocation (INV-3).",
                        request_id,
                        None,
                    ));
                }
                Err(e) => return Err(persistence_error_to_failure(&e, request_id)),
            };

        let mut published = Vec::with_capacity(planned_ports.len());
        let mut port_conflict = false;
        for item in &planned_ports {
            match publish_allocation_port(
                pool,
                allocation.id,
                item.mapping_id,
                candidate.id,
                item.protocol,
                i32::from(item.public_port),
            )
            .await
            {
                Ok(port_row) => published.push(port_row),
                Err(_) => {
                    port_conflict = true;
                    break;
                }
            }
        }

        if port_conflict {
            let _ = release_allocation(pool, allocation.id).await;
            continue;
        }

        return Ok((candidate, allocation, published));
    }

    Err(ApiFailure::new(
        ApiErrorCode::RelayUnavailable,
        "No healthy relay had free public ports for all enabled mappings.",
        request_id,
        None,
    ))
}

#[allow(clippy::too_many_arguments)]
fn assemble_start_tunnel_response(
    key_ring: &SigningKeyRing,
    config: &StartTunnelConfig,
    tunnel: &DbTunnel,
    relay_node: &RelayNode,
    allocation: &RelayAllocation,
    mappings: &[TunnelMapping],
    ports: &[AllocationPort],
    now: DateTime<Utc>,
    request_id: &str,
) -> Result<StartTunnelResponse, ApiFailure> {
    let token_ttl = Duration::seconds(config.token_ttl_seconds.clamp(60, 3600));
    let token_exp = (now + token_ttl).min(allocation.expires_at);

    let claims = build_allocation_token_claims(&TokenClaimsInput {
        issuer: key_ring.issuer(),
        relay_id: relay_node.id,
        tunnel_id: tunnel.id,
        allocation_id: allocation.id,
        mappings,
        ports,
        now,
        exp: token_exp,
        jti: Uuid::new_v4(),
    })?;

    let allocation_token = key_ring
        .sign_allocation_token(&claims)
        .map_err(|e| token_sign_error_to_failure(&e, request_id))?;

    let port_dtos: Vec<AllocatedPortDto> = claims
        .mappings
        .iter()
        .map(|m| AllocatedPortDto {
            mapping_key: m.mapping_key.clone(),
            protocol: m.protocol,
            public_port: m.public_port,
        })
        .collect();

    let response = StartTunnelResponse {
        tunnel_id: tunnel.id.to_string(),
        allocation_token,
        allocation: AllocationDto {
            id: allocation.id.to_string(),
            expires_at: format_rfc3339_utc(allocation.expires_at),
            relay: RelayEndpointDto {
                region: relay_node.region.clone(),
                host: relay_node.public_host.clone(),
                tunnel_port: config.relay_tunnel_port.max(1),
            },
            ports: port_dtos,
        },
    };

    response.validate().map_err(|_| {
        ApiFailure::new(
            ApiErrorCode::Internal,
            "Assembled StartTunnelResponse failed schema validation.",
            request_id,
            None,
        )
    })?;

    Ok(response)
}

#[allow(clippy::too_many_arguments)]
async fn store_start_idempotency_if_present(
    pool: &PgPool,
    caller_id: Uuid,
    idem_key: Option<&str>,
    req_hash: &[u8; 32],
    dto: &StartTunnelResponse,
    now: DateTime<Utc>,
    idempotency_ttl_seconds: i64,
    request_id: &str,
) -> Result<(), ApiFailure> {
    let Some(key) = idem_key else {
        return Ok(());
    };
    let body_value = serde_json::to_value(dto).map_err(|_| {
        ApiFailure::new(
            ApiErrorCode::Internal,
            "Failed to serialize start tunnel response.",
            request_id,
            None,
        )
    })?;
    let idem_expires = now + Duration::seconds(idempotency_ttl_seconds.max(60));
    store_idempotent_response(
        pool,
        &IdempotentResponse {
            user_id: caller_id,
            route: ROUTE_START_TUNNEL,
            idem_key: key,
            request_hash: req_hash,
            status: 200,
            body: &body_value,
            expires_at: idem_expires,
        },
    )
    .await
    .map_err(|e| persistence_error_to_failure(&e, request_id))?;
    Ok(())
}

fn token_sign_error_to_failure(err: &TokenSignError, request_id: &str) -> ApiFailure {
    let _ = err;
    ApiFailure::new(
        ApiErrorCode::Internal,
        "Failed to sign allocation token.",
        request_id,
        None,
    )
}

/// Convert a database [`DbTunnel`] + its [`TunnelMapping`] rows into a schema-validated
/// [`TunnelDto`] (`contracts/tunnel.schema.json`).
pub fn assemble_tunnel_dto(
    tunnel: &DbTunnel,
    mappings: &[TunnelMapping],
    request_id: &str,
) -> Result<TunnelDto, ApiFailure> {
    let state = match tunnel.state.as_str() {
        "created" => WireTunnelState::Created,
        "active" => WireTunnelState::Active,
        "stopped" => WireTunnelState::Stopped,
        "terminated" => WireTunnelState::Terminated,
        "expired" => WireTunnelState::Expired,
        _ => {
            return Err(ApiFailure::new(
                ApiErrorCode::Internal,
                "Stored tunnel state is invalid.",
                request_id,
                None,
            ));
        }
    };

    let mut mapping_dtos = Vec::with_capacity(mappings.len());
    for m in mappings {
        let protocol = match m.protocol.as_str() {
            "tcp" => WireProtocol::Tcp,
            "udp" => WireProtocol::Udp,
            _ => {
                return Err(ApiFailure::new(
                    ApiErrorCode::Internal,
                    "Stored mapping protocol is invalid.",
                    request_id,
                    None,
                ));
            }
        };
        let position = u8::try_from(m.position).map_err(|_| {
            ApiFailure::new(
                ApiErrorCode::Internal,
                "Stored mapping position is out of bounds.",
                request_id,
                None,
            )
        })?;
        mapping_dtos.push(TunnelMappingDto {
            mapping_key: m.mapping_key.clone(),
            protocol,
            position,
            enabled: m.enabled,
        });
    }

    let dto = TunnelDto {
        id: tunnel.id.to_string(),
        name: tunnel.name.clone(),
        game_profile_id: tunnel.game_profile_id.clone(),
        profile_version: tunnel.profile_version,
        state,
        mappings: mapping_dtos,
        created_at: format_rfc3339_utc(tunnel.created_at),
        started_at: tunnel.started_at.map(format_rfc3339_utc),
        stopped_at: tunnel.stopped_at.map(format_rfc3339_utc),
        expires_at: format_rfc3339_utc(tunnel.expires_at),
        termination_reason: tunnel.termination_reason.clone(),
    };

    // Self-check against `contracts/tunnel.schema.json` rules before returning.
    dto.validate().map_err(|_| {
        ApiFailure::new(
            ApiErrorCode::Internal,
            "Assembled tunnel response failed schema validation.",
            request_id,
            None,
        )
    })?;

    Ok(dto)
}

fn format_rfc3339_utc(dt: chrono::DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn policy_error_to_failure(code: PolicyErrorCode, request_id: &str) -> ApiFailure {
    let (api_code, msg) = match code {
        PolicyErrorCode::Unauthenticated => {
            (ApiErrorCode::Unauthenticated, "Authentication required.")
        }
        PolicyErrorCode::Forbidden => (ApiErrorCode::Forbidden, "Operation is not permitted."),
        PolicyErrorCode::NotFound => (ApiErrorCode::NotFound, "Resource not found."),
        PolicyErrorCode::ValidationFailed => {
            (ApiErrorCode::ValidationFailed, "Request validation failed.")
        }
        PolicyErrorCode::Conflict => (ApiErrorCode::Conflict, "Resource state conflict."),
    };
    ApiFailure::new(api_code, msg, request_id, None)
}

fn validation_error_to_failure(err: &ValidationError, request_id: &str) -> ApiFailure {
    match err {
        ValidationError::LocalTargetField(field) => ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Local destination fields are never accepted by the control plane (INV-1).",
            request_id,
            Some(vec![ErrorDetail {
                field: field.clone(),
                issue: "host-local target fields are forbidden (INV-1)".to_owned(),
            }]),
        ),
        ValidationError::InvalidField { field, issue } => ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Request body is invalid.",
            request_id,
            Some(vec![ErrorDetail {
                field: (*field).to_owned(),
                issue: (*issue).to_owned(),
            }]),
        ),
        ValidationError::JsonTooDeep => ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Request JSON nesting exceeds maximum depth.",
            request_id,
            None,
        ),
    }
}

fn persistence_error_to_failure(err: &PersistenceError, request_id: &str) -> ApiFailure {
    match err {
        PersistenceError::Rejected(rejection) => match rejection {
            Rejection::LocalTargetField(field) => ApiFailure::new(
                ApiErrorCode::ValidationFailed,
                "Local destination fields are never accepted by the control plane (INV-1).",
                request_id,
                Some(vec![ErrorDetail {
                    field: field.clone(),
                    issue: "host-local target fields are forbidden (INV-1)".to_owned(),
                }]),
            ),
            Rejection::ProfileProtocolMismatch {
                mapping_key,
                expected,
                actual,
            } => ApiFailure::new(
                ApiErrorCode::ValidationFailed,
                "Mapping protocol does not match the game profile (INV-2).",
                request_id,
                Some(vec![ErrorDetail {
                    field: format!("mappings.{mapping_key}.protocol"),
                    issue: format!("expected {expected}, got {actual}"),
                }]),
            ),
            Rejection::ProfileMappingUnknown { mapping_key, .. } => ApiFailure::new(
                ApiErrorCode::ValidationFailed,
                "Mapping key is not declared by the game profile (INV-2).",
                request_id,
                Some(vec![ErrorDetail {
                    field: format!("mappings.{mapping_key}"),
                    issue: "mapping_key is not defined by this profile".to_owned(),
                }]),
            ),
            Rejection::ProfileSpecMismatch { .. } => ApiFailure::new(
                ApiErrorCode::ValidationFailed,
                "Tunnel profile version mismatch (INV-2).",
                request_id,
                None,
            ),
            Rejection::MappingNotInAllocationTunnel { .. } => ApiFailure::new(
                ApiErrorCode::ValidationFailed,
                "Mapping does not belong to the allocation's tunnel (INV-4).",
                request_id,
                None,
            ),
            Rejection::AuditMetadata(_) => ApiFailure::new(
                ApiErrorCode::ValidationFailed,
                "Audit metadata rejected (INV-5).",
                request_id,
                None,
            ),
            Rejection::Field(issue) => ApiFailure::new(
                ApiErrorCode::ValidationFailed,
                "Field validation failed.",
                request_id,
                Some(vec![ErrorDetail {
                    field: "request".to_owned(),
                    issue: (*issue).to_owned(),
                }]),
            ),
        },
        PersistenceError::Database(_) => ApiFailure::new(
            ApiErrorCode::Internal,
            "An internal database error occurred.",
            request_id,
            None,
        ),
    }
}
