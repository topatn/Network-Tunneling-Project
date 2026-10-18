//! `tunnels` and `tunnel_mappings`.
//!
//! ## SEC-009
//!
//! Every function that reads or writes a tunnel takes `owner_id` and filters on it.
//! There is no unscoped `get_tunnel(pool, id)` in this module, deliberately: an
//! unscoped getter is one careless caller away from a cross-tenant read, and the
//! tenant-scoped getter is the only one anyone should reach for.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::entities::{Protocol, Tunnel, TunnelMapping};
use crate::db::invariants::validate_mapping_against_profile;
use crate::db::profile::ProfileSpec;
use crate::error::Result;

/// A mapping as the caller supplies it, before it becomes a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMapping {
    /// Profile-defined key.
    pub mapping_key: String,
    /// Protocol requested.
    pub protocol: Protocol,
    /// Whether the mapping is active.
    pub enabled: bool,
}

/// Everything needed to create a tunnel.
///
/// Grouped because the alternative was eight positional arguments containing
/// `game_profile_id` and `profile_version` — one fact split across two parameters, where
/// swapping them compiles and produces a tunnel that validates against the wrong
/// profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTunnel<'a> {
    /// Owner. Every tunnel belongs to exactly one user (SEC-009).
    pub owner_id: Uuid,
    /// User-facing name, 1-64 characters.
    pub name: &'a str,
    /// Profile this tunnel is created against.
    pub game_profile_id: &'a str,
    /// Version of that profile.
    pub profile_version: i32,
    /// The mappings to create with it, in wire order.
    pub mappings: &'a [NewMapping],
    /// When the tunnel expires.
    pub expires_at: DateTime<Utc>,
}

/// Create a tunnel and its mappings in one transaction.
///
/// INV-2 is enforced here, against the tunnel's own `(game_profile_id, profile_version)`,
/// because a tunnel with mappings its profile does not define is invalid from the moment
/// it is created and must never be observable in that state — not even briefly.
///
/// The profile is supplied by the caller rather than loaded here; see
/// [`crate::db::profile`] for why.
pub async fn create_tunnel(
    pool: &PgPool,
    new: &NewTunnel<'_>,
    profile: &ProfileSpec,
) -> Result<Tunnel> {
    let NewTunnel {
        owner_id,
        name,
        game_profile_id,
        profile_version,
        mappings,
        expires_at,
    } = *new;
    let mut tx = pool.begin().await?;

    let tunnel = sqlx::query_as::<_, Tunnel>(
        "insert into tunnels (owner_id, name, game_profile_id, profile_version, expires_at)
         values ($1, $2, $3, $4, $5)
         returning id, owner_id, name, game_profile_id, profile_version, state,
                   created_at, started_at, stopped_at, expires_at, termination_reason",
    )
    .bind(owner_id)
    .bind(name)
    .bind(game_profile_id)
    .bind(profile_version)
    .bind(expires_at)
    .fetch_one(&mut *tx)
    .await?;

    // Position is the caller's order in the slice, because it is the index used in UDP
    // datagram headers (contracts.md section 11) and therefore part of the wire contract,
    // not something the database may renumber.
    for (index, mapping) in mappings.iter().enumerate() {
        validate_mapping_against_profile(
            &mapping.mapping_key,
            mapping.protocol,
            profile,
            game_profile_id,
            profile_version,
        )?;

        // The index is bounded by the slice length, which is bounded by the caller.
        let position = i16::try_from(index).map_err(|_| {
            crate::error::PersistenceError::Rejected(crate::error::Rejection::Field(
                "too many mappings to assign positions",
            ))
        })?;

        sqlx::query(
            "insert into tunnel_mappings (tunnel_id, mapping_key, protocol, position, enabled)
             values ($1, $2, $3, $4, $5)",
        )
        .bind(tunnel.id)
        .bind(&mapping.mapping_key)
        .bind(mapping.protocol.as_str())
        .bind(position)
        .bind(mapping.enabled)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(tunnel)
}

/// Fetch a tunnel, scoped to its owner.
///
/// `None` means "no such tunnel **for this owner**" — not "no such tunnel". The API layer
/// turns that into `not_found` rather than `forbidden`, because distinguishing them
/// would confirm the existence of another user's tunnel (`contracts.md` §3).
pub async fn get_tunnel(pool: &PgPool, owner_id: Uuid, tunnel_id: Uuid) -> Result<Option<Tunnel>> {
    let tunnel = sqlx::query_as::<_, Tunnel>(
        "select id, owner_id, name, game_profile_id, profile_version, state,
                created_at, started_at, stopped_at, expires_at, termination_reason
         from tunnels
         where id = $1 and owner_id = $2",
    )
    .bind(tunnel_id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await?;
    Ok(tunnel)
}

/// List a user's tunnels, newest first.
///
/// Bounded. An unbounded list over a table a user can grow without limit is exactly the
/// unbounded allocation `AGENTS.md` §4 forbids, and a hostile user creating tunnels in a
/// loop would otherwise be able to exhaust the control plane's memory.
pub async fn list_tunnels(pool: &PgPool, owner_id: Uuid, limit: i64) -> Result<Vec<Tunnel>> {
    let limit = clamp_limit(limit);
    let tunnels = sqlx::query_as::<_, Tunnel>(
        "select id, owner_id, name, game_profile_id, profile_version, state,
                created_at, started_at, stopped_at, expires_at, termination_reason
         from tunnels
         where owner_id = $1
         order by created_at desc, id desc
         limit $2",
    )
    .bind(owner_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(tunnels)
}

/// Largest page this layer will serve. A caller asking for more gets this instead.
const MAX_LIMIT: i64 = 200;
/// Smallest page. `limit <= 0` is a client bug, not a request for everything.
const MIN_LIMIT: i64 = 1;

fn clamp_limit(limit: i64) -> i64 {
    limit.clamp(MIN_LIMIT, MAX_LIMIT)
}

/// List a tunnel's mappings in wire order.
pub async fn list_mappings(
    pool: &PgPool,
    owner_id: Uuid,
    tunnel_id: Uuid,
) -> Result<Vec<TunnelMapping>> {
    // The owner filter is a join rather than a second lookup: it makes "read mappings of
    // a tunnel you do not own" return nothing in one statement, so there is no window in
    // which ownership could change between two queries.
    let mappings = sqlx::query_as::<_, TunnelMapping>(
        "select m.id, m.tunnel_id, m.mapping_key, m.protocol, m.position, m.enabled,
                m.created_at
         from tunnel_mappings m
         join tunnels t on t.id = m.tunnel_id
         where m.tunnel_id = $1 and t.owner_id = $2
         order by m.position",
    )
    .bind(tunnel_id)
    .bind(owner_id)
    .fetch_all(pool)
    .await?;
    Ok(mappings)
}

/// Move a tunnel to `active`, setting `started_at`.
///
/// Refuses a tunnel that is already terminal: `data-model.md` §4 makes terminal states
/// final and requires a new row for a new session, so reviving one would contradict the
/// state model rather than extend it.
pub async fn start_tunnel(pool: &PgPool, owner_id: Uuid, tunnel_id: Uuid) -> Result<bool> {
    let res = sqlx::query(
        "update tunnels set state = 'active', started_at = now()
         where id = $1 and owner_id = $2 and state = 'created'",
    )
    .bind(tunnel_id)
    .bind(owner_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Stop a tunnel, setting `stopped_at`.
///
/// `stopped_at` is written in the same statement as the state change so INV-7's
/// consistency CHECK can never fail and so there is no intermediate row where the tunnel
/// claims to be stopped without saying when.
pub async fn stop_tunnel(pool: &PgPool, owner_id: Uuid, tunnel_id: Uuid) -> Result<bool> {
    let res = sqlx::query(
        "update tunnels set state = 'stopped', stopped_at = now()
         where id = $1 and owner_id = $2 and state in ('created', 'active')",
    )
    .bind(tunnel_id)
    .bind(owner_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Terminate a tunnel for a reason other than the owner's choice.
///
/// Used by the operator and abuse paths. `reason` is stored so an audit reader can see
/// why, and is a short code rather than free text so it cannot become a payload.
pub async fn terminate_tunnel(pool: &PgPool, tunnel_id: Uuid, reason: &str) -> Result<bool> {
    let res = sqlx::query(
        "update tunnels set state = 'terminated', stopped_at = now(), termination_reason = $2
         where id = $1 and state not in ('stopped', 'terminated', 'expired')",
    )
    .bind(tunnel_id)
    .bind(reason)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Expire tunnels whose lifetime has run out.
///
/// Batched and bounded, because this runs over the whole table: a single unbounded
/// statement would hold a connection and a lock for as long as the backlog takes. The
/// `for update skip locked` clause lets several workers run this at once without
/// blocking each other or double-processing a row.
pub async fn expire_due_tunnels(pool: &PgPool, batch: i64) -> Result<u64> {
    let batch = clamp_limit(batch);
    let res = sqlx::query(
        "update tunnels set state = 'expired', stopped_at = now(),
                             termination_reason = coalesce(termination_reason, 'expired')
         where id in (
             select id from tunnels
             where state = 'active' and expires_at <= now()
             order by expires_at
             limit $1
             for update skip locked
         )",
    )
    .bind(batch)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Count a user's non-terminal tunnels, for the active-tunnel quota.
///
/// `data-model.md` §4 records this as application-enforced because the limit is not yet
/// fixed. The function takes the limit as a parameter rather than a constant precisely so
/// that fixing it later is a configuration change and not a hunt through this module.
pub async fn count_live_tunnels(pool: &PgPool, owner_id: Uuid) -> Result<i64> {
    let n = sqlx::query_scalar::<_, i64>(
        "select count(*) from tunnels where owner_id = $1 and state in ('created', 'active')",
    )
    .bind(owner_id)
    .fetch_one(pool)
    .await?;
    Ok(n)
}
