//! Retention configuration and scheduled cleanup jobs (`T-109`, `PRIV-003`, `PRIV-004`).
//!
//! Guarantees:
//! 1. **Bounded batches (`AGENTS.md` §4):** Deletion queries clamp batches (`1..=1000`)
//!    and order by timestamp to avoid lock starvation and excessive statement duration.
//! 2. **Cascading consistency:** Deletion of terminal tunnels cascades to child mappings,
//!    allocations, ports, invitations, and usage records. Purging soft-deleted users cascades
//!    to all user-owned rows.
//! 3. **Audit data minimization (`PRIV-002`, `INV-5`):** Retention jobs log only counts and durations;
//!    they never log guest IP addresses, tokens, or personal identifiers.
//! 4. **Clock consistency:** Expired timestamps are evaluated against database statement time
//!    or caller-supplied UTC cutoff.

use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;

use crate::error::Result;

/// Default retention configuration per `requirements.md` (`PRIV-003`, `T-109`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicyConfig {
    /// Guest admissions retention after expiry or revocation (`24 hours`).
    pub guest_admissions: Duration,
    /// Idempotency key retention window (`24 hours`).
    pub idempotency_keys: Duration,
    /// Auth session retention after expiry or revocation (`7 days`).
    pub auth_sessions: Duration,
    /// Terminal tunnel retention after `stopped_at` (`30 days`).
    pub terminal_tunnels: Duration,
    /// Usage records retention after `interval_start` (`90 days`).
    pub usage_records: Duration,
    /// Audit events retention after `occurred_at` (`90 days`).
    pub audit_events: Duration,
    /// Soft-deleted user account retention after `deleted_at` (`30 days`).
    pub soft_deleted_users: Duration,
}

impl Default for RetentionPolicyConfig {
    fn default() -> Self {
        Self {
            guest_admissions: Duration::hours(24),
            idempotency_keys: Duration::hours(24),
            auth_sessions: Duration::days(7),
            terminal_tunnels: Duration::days(30),
            usage_records: Duration::days(90),
            audit_events: Duration::days(90),
            soft_deleted_users: Duration::days(30),
        }
    }
}

/// Aggregated metrics from a retention cleanup sweep.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetentionSweepReport {
    /// Expired or revoked guest admissions deleted.
    pub purged_guest_admissions: u64,
    /// Expired idempotency keys deleted.
    pub purged_idempotency_keys: u64,
    /// Expired or revoked auth sessions deleted.
    pub purged_auth_sessions: u64,
    /// Terminal tunnels (and cascaded children) deleted.
    pub purged_terminal_tunnels: u64,
    /// Expired usage records deleted.
    pub purged_usage_records: u64,
    /// Expired audit events deleted.
    pub purged_audit_events: u64,
    /// Soft-deleted user accounts (and cascaded children) deleted.
    pub purged_soft_deleted_users: u64,
}

/// Purge guest admissions that expired or were revoked before `cutoff`.
pub async fn purge_expired_guest_admissions(
    pool: &PgPool,
    cutoff: DateTime<Utc>,
    batch: i64,
) -> Result<u64> {
    let batch = batch.clamp(1, 1000);
    let res = sqlx::query(
        "delete from guest_admissions
         where id in (
             select id from guest_admissions
             where (expires_at <= $1 or (revoked_at is not null and revoked_at <= $1))
             order by expires_at
             limit $2
         )",
    )
    .bind(cutoff)
    .bind(batch)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Purge expired idempotency keys whose retention window has passed.
pub async fn purge_expired_idempotency_keys(pool: &PgPool, batch: i64) -> Result<u64> {
    crate::db::ops::purge_expired_idempotency_keys(pool, batch).await
}

/// Purge auth sessions that expired or were revoked before `cutoff`.
pub async fn purge_expired_auth_sessions(
    pool: &PgPool,
    cutoff: DateTime<Utc>,
    batch: i64,
) -> Result<u64> {
    let batch = batch.clamp(1, 1000);
    let res = sqlx::query(
        "delete from auth_sessions
         where id in (
             select id from auth_sessions
             where (expires_at <= $1 or (revoked_at is not null and revoked_at <= $1))
             order by expires_at
             limit $2
         )",
    )
    .bind(cutoff)
    .bind(batch)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Purge terminal tunnels (`stopped`, `terminated`, `expired`) whose `stopped_at` is before `cutoff`.
///
/// Cascades automatically via PostgreSQL foreign keys to `tunnel_mappings`, `relay_allocations`,
/// `allocation_ports`, `invitations`, and `usage_records`.
pub async fn purge_expired_terminal_tunnels(
    pool: &PgPool,
    cutoff: DateTime<Utc>,
    batch: i64,
) -> Result<u64> {
    let batch = batch.clamp(1, 1000);
    let res = sqlx::query(
        "delete from tunnels
         where id in (
             select id from tunnels
             where state in ('stopped', 'terminated', 'expired')
               and stopped_at is not null
               and stopped_at <= $1
             order by stopped_at
             limit $2
         )",
    )
    .bind(cutoff)
    .bind(batch)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Purge usage records whose `interval_start` is before `cutoff`.
pub async fn purge_expired_usage_records(
    pool: &PgPool,
    cutoff: DateTime<Utc>,
    batch: i64,
) -> Result<u64> {
    let batch = batch.clamp(1, 1000);
    let res = sqlx::query(
        "delete from usage_records
         where (tunnel_id, relay_id, interval_start) in (
             select tunnel_id, relay_id, interval_start from usage_records
             where interval_start <= $1
             order by interval_start
             limit $2
         )",
    )
    .bind(cutoff)
    .bind(batch)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Purge audit events that occurred before `cutoff`.
pub async fn purge_expired_audit_events(
    pool: &PgPool,
    cutoff: DateTime<Utc>,
    batch: i64,
) -> Result<u64> {
    let batch = batch.clamp(1, 1000);
    let res = sqlx::query(
        "delete from audit_events
         where id in (
             select id from audit_events
             where occurred_at <= $1
             order by occurred_at
             limit $2
         )",
    )
    .bind(cutoff)
    .bind(batch)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Purge soft-deleted user accounts whose `deleted_at` is before `cutoff` (`PRIV-004`).
///
/// Cascades automatically to all associated sessions, tunnels, mappings, and invitations.
pub async fn purge_soft_deleted_users(
    pool: &PgPool,
    cutoff: DateTime<Utc>,
    batch: i64,
) -> Result<u64> {
    let batch = batch.clamp(1, 1000);
    let res = sqlx::query(
        "delete from users
         where id in (
             select id from users
             where deleted_at is not null
               and deleted_at <= $1
             order by deleted_at
             limit $2
         )",
    )
    .bind(cutoff)
    .bind(batch)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Run a complete retention cleanup sweep across all data categories.
pub async fn run_retention_sweep(
    pool: &PgPool,
    config: &RetentionPolicyConfig,
    now: DateTime<Utc>,
    batch_limit: i64,
) -> Result<RetentionSweepReport> {
    let guest_cutoff = now
        .checked_sub_signed(config.guest_admissions)
        .unwrap_or(now);
    let auth_cutoff = now.checked_sub_signed(config.auth_sessions).unwrap_or(now);
    let tunnel_cutoff = now
        .checked_sub_signed(config.terminal_tunnels)
        .unwrap_or(now);
    let usage_cutoff = now.checked_sub_signed(config.usage_records).unwrap_or(now);
    let audit_cutoff = now.checked_sub_signed(config.audit_events).unwrap_or(now);
    let user_cutoff = now
        .checked_sub_signed(config.soft_deleted_users)
        .unwrap_or(now);

    let purged_admissions = purge_expired_guest_admissions(pool, guest_cutoff, batch_limit).await?;
    let purged_keys = purge_expired_idempotency_keys(pool, batch_limit).await?;
    let purged_sessions = purge_expired_auth_sessions(pool, auth_cutoff, batch_limit).await?;
    let purged_tunnels = purge_expired_terminal_tunnels(pool, tunnel_cutoff, batch_limit).await?;
    let purged_usage = purge_expired_usage_records(pool, usage_cutoff, batch_limit).await?;
    let purged_audit = purge_expired_audit_events(pool, audit_cutoff, batch_limit).await?;
    let purged_users = purge_soft_deleted_users(pool, user_cutoff, batch_limit).await?;

    Ok(RetentionSweepReport {
        purged_guest_admissions: purged_admissions,
        purged_idempotency_keys: purged_keys,
        purged_auth_sessions: purged_sessions,
        purged_terminal_tunnels: purged_tunnels,
        purged_usage_records: purged_usage,
        purged_audit_events: purged_audit,
        purged_soft_deleted_users: purged_users,
    })
}
