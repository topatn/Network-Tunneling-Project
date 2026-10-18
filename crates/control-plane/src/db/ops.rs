//! `usage_records`, `audit_events`, and `idempotency_keys`.
//!
//! These three are grouped because they share a property: each is written on behalf of
//! someone else, and each can be turned into a leak or a forgery if that is not handled
//! carefully.

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::db::audit::AuditMetadata;
use crate::db::entities::{ActorType, AuditEvent, IdempotencyRecord, UsageRecord};
use crate::error::{PersistenceError, Rejection, Result};

/// One relay report: how much a tunnel moved during an interval.
///
/// Grouped into a struct because eight positional arguments is past the point where a
/// caller can reliably get the order right — and four of them are integers, so a
/// transposition compiles silently and mis-bills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageDelta {
    /// Tunnel measured.
    pub tunnel_id: Uuid,
    /// Relay that carried the traffic.
    pub relay_id: Uuid,
    /// Start of the interval.
    pub interval_start: DateTime<Utc>,
    /// Bytes sent to guests during the interval.
    pub bytes_from_guests: i64,
    /// Bytes received from guests during the interval.
    pub bytes_to_guests: i64,
    /// Peak concurrent flows observed.
    pub peak_flows: i32,
    /// Datagrams the relay dropped.
    pub datagrams_dropped: i64,
}

/// Add to an aggregated usage bucket for one interval.
///
/// An upsert that **adds**, not one that overwrites. A relay reporting the same interval
/// twice must not erase the earlier figures, and must not double-count either — so the
/// caller sends deltas, and the row is the running total. The alternative, replacing the
/// row, would make a retried report erase usage and quietly under-bill or over-quota.
///
/// Values are clamped at zero here because the column CHECK rejects negatives and a
/// relay that reset a counter would otherwise fail the whole report batch.
pub async fn add_usage(pool: &PgPool, delta: &UsageDelta) -> Result<UsageRecord> {
    let UsageDelta {
        tunnel_id,
        relay_id,
        interval_start,
        bytes_from_guests,
        bytes_to_guests,
        peak_flows,
        datagrams_dropped,
    } = *delta;

    let record = sqlx::query_as::<_, UsageRecord>(
        "insert into usage_records
             (tunnel_id, relay_id, interval_start, bytes_from_guests, bytes_to_guests,
              peak_flows, datagrams_dropped)
         values ($1, $2, $3, greatest($4, 0), greatest($5, 0), greatest($6, 0), greatest($7, 0))
         on conflict (tunnel_id, relay_id, interval_start) do update
         set bytes_from_guests  = usage_records.bytes_from_guests  + greatest(excluded.bytes_from_guests, 0),
             bytes_to_guests    = usage_records.bytes_to_guests    + greatest(excluded.bytes_to_guests, 0),
             peak_flows         = greatest(usage_records.peak_flows, excluded.peak_flows),
             datagrams_dropped  = usage_records.datagrams_dropped  + greatest(excluded.datagrams_dropped, 0)
         returning tunnel_id, relay_id, interval_start, bytes_from_guests, bytes_to_guests,
                   peak_flows, datagrams_dropped",
    )
    .bind(tunnel_id)
    .bind(relay_id)
    .bind(interval_start)
    .bind(bytes_from_guests)
    .bind(bytes_to_guests)
    .bind(peak_flows)
    .bind(datagrams_dropped)
    .fetch_one(pool)
    .await?;
    Ok(record)
}

/// Total bytes for a tunnel since a point, for quota checks.
///
/// **PostgreSQL widens `sum()` over `bigint` to `numeric`,** because a sum of bigints need
/// not fit in a bigint. Decoding the result straight into an `i64` therefore fails with a
/// `ColumnDecode` error, which is what the first run of these tests found.
///
/// The cast is in SQL rather than in Rust because decoding `numeric` would mean adding
/// `rust_decimal` or `bigdecimal` for one aggregate, and `AGENTS.md` §5.2 does not allow a
/// dependency for a few lines of code. The `least(...)` bound is not decoration: without it a
/// hostile relay reporting enormous figures could push the total past `i64` and turn a quota
/// check into a decode failure. Clamping reports the ceiling, which is the answer a quota
/// check wants anyway.
pub async fn usage_since(pool: &PgPool, tunnel_id: Uuid, since: DateTime<Utc>) -> Result<i64> {
    let total = sqlx::query_scalar::<_, i64>(
        "select coalesce(
                    least(sum(bytes_from_guests + bytes_to_guests), 9223372036854775807),
                    0
                )::bigint
         from usage_records where tunnel_id = $1 and interval_start >= $2",
    )
    .bind(tunnel_id)
    .bind(since)
    .fetch_one(pool)
    .await?;
    Ok(total)
}

/// An audit event to append.
///
/// Grouped because the alternative was eight positional arguments, four of them optional
/// `Option`s that are easy to transpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewAuditEvent<'a> {
    /// Who did it.
    pub actor_type: ActorType,
    /// Which identity, when there is one.
    pub actor_id: Option<Uuid>,
    /// What happened. The vocabulary is not yet enumerated in `contracts.md`.
    pub action: &'a str,
    /// What it happened to.
    pub subject_type: &'a str,
    /// Which instance of that subject.
    pub subject_id: Option<Uuid>,
    /// Correlates the writes of one request.
    pub request_id: Option<&'a str>,
    /// Already validated by INV-5; there is no way to build one that has not been.
    pub metadata: &'a AuditMetadata,
}

/// Append an audit event.
///
/// The only way to write one. [`AuditMetadata`] cannot be constructed without passing
/// the INV-5 checks, so there is no path here that reaches the `jsonb` column with an
/// unchecked blob.
///
/// Append-only: there is no update or delete function in this module, and `data-model.md`
/// §4 specifies that audit rows are never updated.
pub async fn append_audit_event(pool: &PgPool, event: &NewAuditEvent<'_>) -> Result<i64> {
    let NewAuditEvent {
        actor_type,
        actor_id,
        action,
        subject_type,
        subject_id,
        request_id,
        metadata,
    } = event;

    let row = sqlx::query(
        "insert into audit_events
             (actor_type, actor_id, action, subject_type, subject_id, request_id, metadata)
         values ($1, $2, $3, $4, $5, $6, $7)
         returning id",
    )
    .bind(actor_type.as_str())
    .bind(actor_id)
    .bind(action)
    .bind(subject_type)
    .bind(subject_id)
    .bind(request_id)
    .bind(metadata.as_value())
    .fetch_one(pool)
    .await?;

    // `RETURNING id` on a bigint identity column. The value is either the identity type
    // or something convertible; treating anything else as an error is better than
    // silently writing an event with no identifier.
    let id: i64 = row.try_get("id").map_err(|e| {
        let _ = e;
        PersistenceError::Rejected(Rejection::Field("audit_events.id was not readable"))
    })?;
    Ok(id)
}

/// Read audit events for one subject, newest first.
///
/// Bounded, and read-only. There is no function here that deletes, so retention is a job
/// outside this module — deliberately, so that pruning cannot be reached from a request
/// handler by mistake.
pub async fn list_audit_events(
    pool: &PgPool,
    subject_type: &str,
    subject_id: Option<Uuid>,
    limit: i64,
) -> Result<Vec<AuditEvent>> {
    let limit = limit.clamp(1, 200);
    let events = sqlx::query_as::<_, AuditEvent>(
        "select id, occurred_at, actor_type, actor_id, action, subject_type, subject_id,
                request_id, metadata
         from audit_events
         where subject_type = $1
           and ($2::uuid is null or subject_id = $2)
         order by occurred_at desc, id desc
         limit $3",
    )
    .bind(subject_type)
    .bind(subject_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(events)
}

/// What a stored idempotency key means for the current request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdempotencyOutcome {
    /// First use of this key. The caller should do the work and store the response.
    Fresh,
    /// Same key, same request: replay this stored response.
    Replay {
        /// Status to return.
        status: i32,
        /// Body to return.
        body: serde_json::Value,
    },
    /// Same key, different request. `contracts.md` §8 requires a conflict, not a replay:
    /// silently returning the earlier response would let one request's result be served
    /// in answer to a different one.
    Conflict,
}

/// Look up an idempotency key and classify it against this request's hash.
///
/// The comparison is on `request_hash`, never on the body, so a large request is hashed
/// once and the key table stays small.
pub async fn check_idempotency(
    pool: &PgPool,
    user_id: Uuid,
    route: &str,
    idem_key: &str,
    request_hash: &[u8],
) -> Result<IdempotencyOutcome> {
    let record = sqlx::query_as::<_, IdempotencyRecord>(
        "select user_id, route, idem_key, request_hash, response_status, response_body,
                created_at, expires_at
         from idempotency_keys
         where user_id = $1 and route = $2 and idem_key = $3",
    )
    .bind(user_id)
    .bind(route)
    .bind(idem_key)
    .fetch_optional(pool)
    .await?;

    let Some(record) = record else {
        return Ok(IdempotencyOutcome::Fresh);
    };

    if record.request_hash != request_hash {
        return Ok(IdempotencyOutcome::Conflict);
    }

    match (record.response_status, record.response_body) {
        (Some(status), Some(body)) => Ok(IdempotencyOutcome::Replay { status, body }),
        // The row exists with no response yet: a concurrent request for the same key is
        // still in flight. Treated as fresh so the caller waits and retries rather than
        // inventing an empty response.
        _ => Ok(IdempotencyOutcome::Fresh),
    }
}

/// A response to store against an idempotency key.
///
/// Grouped for the same reason as [`UsageDelta`]: `user_id`, `route` and `idem_key` are
/// the composite primary key and `status` is an integer, so a transposed call would
/// compile and write the response to the wrong key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdempotentResponse<'a> {
    /// Scoping user.
    pub user_id: Uuid,
    /// Route the key was used on.
    pub route: &'a str,
    /// The client-supplied key.
    pub idem_key: &'a str,
    /// Hash of the request body.
    pub request_hash: &'a [u8],
    /// Status to store.
    pub status: i32,
    /// Body to store.
    pub body: &'a serde_json::Value,
    /// When the key may be forgotten.
    pub expires_at: DateTime<Utc>,
}

/// Store a response against an idempotency key.
///
/// Claiming the key is an `ON CONFLICT DO NOTHING`, so exactly one concurrent request
/// wins and stores the response. A caller that loses the race re-reads and replays
/// instead of overwriting.
pub async fn store_idempotent_response(
    pool: &PgPool,
    stored: &IdempotentResponse<'_>,
) -> Result<bool> {
    let IdempotentResponse {
        user_id,
        route,
        idem_key,
        request_hash,
        status,
        body,
        expires_at,
    } = stored;

    let res = sqlx::query(
        "insert into idempotency_keys
             (user_id, route, idem_key, request_hash, response_status, response_body, expires_at)
         values ($1, $2, $3, $4, $5, $6, $7)
         on conflict (user_id, route, idem_key) do nothing",
    )
    .bind(user_id)
    .bind(route)
    .bind(idem_key)
    .bind(request_hash)
    .bind(status)
    .bind(body)
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Delete expired idempotency keys. Returns the count, not the rows.
pub async fn purge_expired_idempotency_keys(pool: &PgPool, batch: i64) -> Result<u64> {
    let batch = batch.clamp(1, 1000);
    let res = sqlx::query(
        "delete from idempotency_keys
         where (user_id, route, idem_key) in (
             select user_id, route, idem_key from idempotency_keys
             where expires_at <= now() order by expires_at limit $1
         )",
    )
    .bind(batch)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}
