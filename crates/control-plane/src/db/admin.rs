//! Admin identity and session persistence (`T-049`, `SEC-018`, ADR-019).
//!
//! Administrator identity is strictly decoupled from `users` (`data-model.md` §2, ADR-019).
//! An admin row never holds tunnels, and user sessions carry no admin authorization.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::entities::{Admin, AdminSession};
use crate::error::Result;

/// Register the sole administrator identity.
pub async fn create_admin(
    pool: &PgPool,
    username: &str,
    display_name: &str,
    passkey_credential_id: &[u8],
    passkey_public_key: &[u8],
    recovery_code_hash: &str,
) -> Result<Admin> {
    let row = sqlx::query_as::<_, Admin>(
        "insert into admins (username, display_name, passkey_credential_id, passkey_public_key, recovery_code_hash)
         values ($1, $2, $3, $4, $5)
         returning id, username, display_name, passkey_credential_id, passkey_public_key,
                   passkey_counter, recovery_code_hash, recovery_used_at, created_at, updated_at",
    )
    .bind(username)
    .bind(display_name)
    .bind(passkey_credential_id)
    .bind(passkey_public_key)
    .bind(recovery_code_hash)
    .fetch_one(pool)
    .await?;

    Ok(row)
}

/// Retrieve administrator by username.
pub async fn get_admin_by_username(pool: &PgPool, username: &str) -> Result<Option<Admin>> {
    let row = sqlx::query_as::<_, Admin>(
        "select id, username, display_name, passkey_credential_id, passkey_public_key,
                passkey_counter, recovery_code_hash, recovery_used_at, created_at, updated_at
         from admins where username = $1",
    )
    .bind(username)
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

/// Retrieve administrator by ID.
pub async fn get_admin(pool: &PgPool, admin_id: Uuid) -> Result<Option<Admin>> {
    let row = sqlx::query_as::<_, Admin>(
        "select id, username, display_name, passkey_credential_id, passkey_public_key,
                passkey_counter, recovery_code_hash, recovery_used_at, created_at, updated_at
         from admins where id = $1",
    )
    .bind(admin_id)
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

/// Monotonically increment/update the passkey signature counter for clone detection (`SEC-018`).
pub async fn update_passkey_counter(
    pool: &PgPool,
    admin_id: Uuid,
    new_counter: i64,
) -> Result<bool> {
    let res = sqlx::query(
        "update admins
         set passkey_counter = $2, updated_at = now()
         where id = $1 and passkey_counter < $2",
    )
    .bind(admin_id)
    .bind(new_counter)
    .execute(pool)
    .await?;

    Ok(res.rows_affected() > 0)
}

/// Invalidate offline recovery code upon single use (`SEC-018`).
pub async fn consume_recovery_code(pool: &PgPool, admin_id: Uuid) -> Result<bool> {
    let res = sqlx::query(
        "update admins
         set recovery_used_at = now(), updated_at = now()
         where id = $1 and recovery_used_at is null",
    )
    .bind(admin_id)
    .execute(pool)
    .await?;

    Ok(res.rows_affected() > 0)
}

/// Create a new administrative session.
pub async fn create_admin_session(
    pool: &PgPool,
    admin_id: Uuid,
    token_hash: &str,
    expires_at: DateTime<Utc>,
) -> Result<AdminSession> {
    let row = sqlx::query_as::<_, AdminSession>(
        "insert into admin_sessions (admin_id, token_hash, expires_at)
         values ($1, $2, $3)
         returning id, admin_id, token_hash, created_at, expires_at, step_up_until, revoked_at",
    )
    .bind(admin_id)
    .bind(token_hash)
    .bind(expires_at)
    .fetch_one(pool)
    .await?;

    Ok(row)
}

/// Lookup active administrative session by hashed token.
pub async fn get_admin_session_by_token_hash(
    pool: &PgPool,
    token_hash: &str,
) -> Result<Option<AdminSession>> {
    let row = sqlx::query_as::<_, AdminSession>(
        "select id, admin_id, token_hash, created_at, expires_at, step_up_until, revoked_at
         from admin_sessions
         where token_hash = $1 and revoked_at is null and expires_at > now()",
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

/// Update step-up re-authentication expiration for destructive actions (`contracts.md` §9.1).
pub async fn update_admin_session_step_up(
    pool: &PgPool,
    session_id: Uuid,
    step_up_until: DateTime<Utc>,
) -> Result<bool> {
    let res = sqlx::query(
        "update admin_sessions
         set step_up_until = $2
         where id = $1 and revoked_at is null and expires_at > now()",
    )
    .bind(session_id)
    .bind(step_up_until)
    .execute(pool)
    .await?;

    Ok(res.rows_affected() > 0)
}

/// Revoke an administrative session.
pub async fn revoke_admin_session(pool: &PgPool, session_id: Uuid) -> Result<bool> {
    let res = sqlx::query(
        "update admin_sessions
         set revoked_at = now()
         where id = $1 and revoked_at is null",
    )
    .bind(session_id)
    .execute(pool)
    .await?;

    Ok(res.rows_affected() > 0)
}

/// Purge expired administrative sessions past the retention cutoff (`PRIV-003`).
pub async fn purge_expired_admin_sessions(pool: &PgPool, cutoff: DateTime<Utc>) -> Result<u64> {
    let res = sqlx::query(
        "delete from admin_sessions
         where (revoked_at is not null and revoked_at < $1)
            or (expires_at < $1)",
    )
    .bind(cutoff)
    .execute(pool)
    .await?;

    Ok(res.rows_affected())
}
