//! `users` and `auth_sessions`.
//!
//! Every function here is keyed by an explicit id. Nothing takes "the current user" from
//! ambient state, because SEC-009 requires that a caller cannot reach another user's
//! rows and the only reliable way to guarantee that is for the scoping to be visible in
//! the signature.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::entities::{AuthSession, User, UserStatus};
use crate::error::Result;

/// Create a user, or return the existing row for the same identity.
///
/// Idempotent on `(idp_issuer, idp_subject)` because sign-in is not a create event: a
/// returning user must not get a second row, and the unique index is what makes a race
/// between two concurrent first-sign-ins safe.
///
/// The `ON CONFLICT DO NOTHING` + re-select shape is deliberate. `ON CONFLICT DO UPDATE`
/// would need a list of columns to update, and any choice of column set would
/// reintroduce exactly the "last writer wins" behaviour that lets one request silently
/// overwrite another.
pub async fn upsert_user(
    pool: &PgPool,
    idp_issuer: &str,
    idp_subject: &str,
    display_name: Option<&str>,
) -> Result<User> {
    let inserted = sqlx::query_as::<_, User>(
        "insert into users (idp_issuer, idp_subject, display_name)
         values ($1, $2, $3)
         on conflict (idp_issuer, idp_subject) do nothing
         returning id, idp_issuer, idp_subject, display_name, status, created_at, deleted_at",
    )
    .bind(idp_issuer)
    .bind(idp_subject)
    .bind(display_name)
    .fetch_optional(pool)
    .await?;

    match inserted {
        Some(user) => Ok(user),
        None => {
            // Lost the race, or this is a returning user. Read the winner's row.
            get_user_by_identity(pool, idp_issuer, idp_subject)
                .await?
                // `fetch_optional` returning None here would mean the row vanished between
                // the insert and the read, which is only possible via the deletion job.
                // Surfacing that as a rejection is more honest than inventing a user.
                .ok_or(crate::error::PersistenceError::Rejected(
                    crate::error::Rejection::Field("user disappeared during upsert"),
                ))
        }
    }
}

/// Fetch a user by its identity-provider pair.
pub async fn get_user_by_identity(
    pool: &PgPool,
    idp_issuer: &str,
    idp_subject: &str,
) -> Result<Option<User>> {
    let user = sqlx::query_as::<_, User>(
        "select id, idp_issuer, idp_subject, display_name, status, created_at, deleted_at
         from users
         where idp_issuer = $1 and idp_subject = $2",
    )
    .bind(idp_issuer)
    .bind(idp_subject)
    .fetch_optional(pool)
    .await?;
    Ok(user)
}

/// Fetch a user by primary key.
///
/// Returns `None` for a soft-deleted user as well as a missing one. A soft-deleted
/// account must not be usable, and the caller has no business distinguishing the two.
pub async fn get_user(pool: &PgPool, user_id: Uuid) -> Result<Option<User>> {
    let user = sqlx::query_as::<_, User>(
        "select id, idp_issuer, idp_subject, display_name, status, created_at, deleted_at
         from users
         where id = $1 and deleted_at is null",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(user)
}

/// Suspend or reinstate a user.
///
/// `data-model.md` §4: suspending terminates the user's existing active tunnels. That is
/// done in the same transaction so the account can never be observed suspended with its
/// tunnels still running, which is the window an attacker would use.
pub async fn set_user_status(
    pool: &PgPool,
    user_id: Uuid,
    status: UserStatus,
    reason: Option<&str>,
) -> Result<u64> {
    let mut tx = pool.begin().await?;

    let updated = sqlx::query("update users set status = $1 where id = $2 and deleted_at is null")
        .bind(status.as_str())
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    if updated.rows_affected() == 0 {
        // Roll back rather than commit an empty transaction, so a caller that forgot to
        // check cannot believe it suspended someone.
        tx.rollback().await?;
        return Ok(0);
    }

    if status == UserStatus::Suspended {
        sqlx::query(
            "update tunnels
             set state = 'terminated',
                 stopped_at = now(),
                 termination_reason = coalesce($2, 'owner suspended')
             where owner_id = $1 and state in ('created', 'active')",
        )
        .bind(user_id)
        .bind(reason)
        .execute(&mut *tx)
        .await?;

        // Refresh tokens for a suspended account must stop working immediately, or the
        // suspension would be defeatable with a token issued before it.
        sqlx::query(
            "update auth_sessions set revoked_at = now() where user_id = $1 and revoked_at is null",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(updated.rows_affected())
}

/// Create a refresh-token session.
///
/// `refresh_token_hash` is a hash, never the token. Passing a raw token here would put a
/// credential in the database, which `AGENTS.md` §7 forbids outright.
pub async fn create_session(
    pool: &PgPool,
    user_id: Uuid,
    family_id: Uuid,
    refresh_token_hash: &[u8],
    device_label: Option<&str>,
    expires_at: DateTime<Utc>,
) -> Result<AuthSession> {
    let session = sqlx::query_as::<_, AuthSession>(
        "insert into auth_sessions
             (user_id, family_id, refresh_token_hash, device_label, expires_at)
         values ($1, $2, $3, $4, $5)
         returning id, user_id, family_id, refresh_token_hash, device_label,
                   created_at, last_used_at, expires_at, revoked_at",
    )
    .bind(user_id)
    .bind(family_id)
    .bind(refresh_token_hash)
    .bind(device_label)
    .bind(expires_at)
    .fetch_one(pool)
    .await?;
    Ok(session)
}

/// Look up a session by its token hash, for rotation.
///
/// Returns `None` when the token is unknown **or** already revoked. A caller must not be
/// able to tell those apart, because that difference is what tells an attacker whether a
/// guessed token ever existed.
pub async fn get_active_session_by_token_hash(
    pool: &PgPool,
    refresh_token_hash: &[u8],
) -> Result<Option<AuthSession>> {
    let session = sqlx::query_as::<_, AuthSession>(
        "select id, user_id, family_id, refresh_token_hash, device_label,
                created_at, last_used_at, expires_at, revoked_at
         from auth_sessions
         where refresh_token_hash = $1 and revoked_at is null",
    )
    .bind(refresh_token_hash)
    .fetch_optional(pool)
    .await?;
    Ok(session)
}

/// Record a session's use, for absolute-lifetime enforcement.
///
/// Advances `last_used_at` only. It must not extend `expires_at`: `contracts.md` §9 caps
/// the absolute lifetime regardless of activity, and a sliding expiry here would quietly
/// remove that cap.
pub async fn touch_session(pool: &PgPool, session_id: Uuid, at: DateTime<Utc>) -> Result<bool> {
    let res = sqlx::query(
        "update auth_sessions set last_used_at = $2
         where id = $1 and revoked_at is null and expires_at > $2",
    )
    .bind(session_id)
    .bind(at)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Revoke one session.
pub async fn revoke_session(pool: &PgPool, session_id: Uuid) -> Result<bool> {
    let res = sqlx::query(
        "update auth_sessions set revoked_at = now()
         where id = $1 and revoked_at is null",
    )
    .bind(session_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Revoke every session in a rotation family.
///
/// This is reuse detection (`data-model.md` §4): presenting a token that has already
/// been rotated means the token leaked, so the whole family dies rather than just the
/// presented one.
pub async fn revoke_session_family(pool: &PgPool, family_id: Uuid) -> Result<u64> {
    let res = sqlx::query(
        "update auth_sessions set revoked_at = now()
         where family_id = $1 and revoked_at is null",
    )
    .bind(family_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Revoke every live session belonging to a user, scoped by owner.
///
/// Scoped so a caller holding one user's id cannot revoke another's sessions (SEC-009).
pub async fn revoke_user_sessions(pool: &PgPool, user_id: Uuid) -> Result<u64> {
    let res = sqlx::query(
        "update auth_sessions set revoked_at = now()
         where user_id = $1 and revoked_at is null",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}
