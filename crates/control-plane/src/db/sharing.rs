//! `invitations` and `guest_admissions`.
//!
//! ADR-007 chose G2: a guest connects with the stock Minecraft client, with no guest
//! account and no guest client. So `guest_admissions` exists to hold the **source-IP
//! grant the relay enforces**, not an identity — and that is exactly why the table is
//! load-bearing under G2 and why E-13 must answer before it can be relied on.
//!
//! `source_ip` is personal data (`data-model.md` §4). It lives here because the relay
//! needs it, and it must never be copied into a log, an audit event, or an error message.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::entities::{GuestAdmission, Invitation};
use crate::error::{PersistenceError, Rejection, Result};

/// Create an invitation.
///
/// `token_hash` is a hash. The plaintext secret is shown to the creator once and is not
/// recoverable afterwards, so passing the secret itself here would create a credential
/// at rest that `AGENTS.md` §7 forbids.
pub async fn create_invitation(
    pool: &PgPool,
    tunnel_id: Uuid,
    created_by: Uuid,
    token_hash: &[u8],
    max_claims: i32,
    expires_at: DateTime<Utc>,
) -> Result<Invitation> {
    let invitation = sqlx::query_as::<_, Invitation>(
        "insert into invitations (tunnel_id, created_by, token_hash, max_claims, expires_at)
         values ($1, $2, $3, $4, $5)
         returning id, tunnel_id, created_by, token_hash, max_claims, claim_count,
                   created_at, expires_at, revoked_at",
    )
    .bind(tunnel_id)
    .bind(created_by)
    .bind(token_hash)
    .bind(max_claims)
    .bind(expires_at)
    .fetch_one(pool)
    .await?;
    Ok(invitation)
}

/// Claim an invitation.
///
/// One statement, so the check and the increment cannot race. A read-then-write version
/// would let two guests both see `claim_count < max_claims` and both be admitted.
///
/// Returns `None` when the invitation is unusable — unknown, revoked, expired, or
/// exhausted. The caller learns nothing about *which*, because that difference would let
/// someone probe an invitation's state (`contracts.md` §3: `not_found`, not `forbidden`).
pub async fn claim_invitation(
    pool: &PgPool,
    token_hash: &[u8],
    at: DateTime<Utc>,
) -> Result<Option<Invitation>> {
    let claimed = sqlx::query_as::<_, Invitation>(
        "update invitations
         set claim_count = claim_count + 1
         where token_hash = $1
           and revoked_at is null
           and expires_at > $2
           and claim_count < max_claims
         returning id, tunnel_id, created_by, token_hash, max_claims, claim_count,
                   created_at, expires_at, revoked_at",
    )
    .bind(token_hash)
    .bind(at)
    .fetch_optional(pool)
    .await?;
    Ok(claimed)
}

/// Revoke an invitation and drop the admissions it granted.
///
/// One transaction: revoking the invitation without revoking its admissions would leave
/// guests admitted by a credential that no longer works.
pub async fn revoke_invitation(pool: &PgPool, owner_id: Uuid, invitation_id: Uuid) -> Result<bool> {
    let mut tx = pool.begin().await?;

    // Scoped by owner through the tunnel, so one user cannot revoke another's invitation.
    let res = sqlx::query(
        "update invitations set revoked_at = now()
         where id = $1 and revoked_at is null
           and exists (select 1 from tunnels t where t.id = invitations.tunnel_id
                       and t.owner_id = $2)",
    )
    .bind(invitation_id)
    .bind(owner_id)
    .execute(&mut *tx)
    .await?;

    if res.rows_affected() > 0 {
        sqlx::query(
            "update guest_admissions set revoked_at = now()
             where invitation_id = $1 and revoked_at is null",
        )
        .bind(invitation_id)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}

/// List a user's invitations for one of their tunnels.
pub async fn list_invitations(
    pool: &PgPool,
    owner_id: Uuid,
    tunnel_id: Uuid,
) -> Result<Vec<Invitation>> {
    let invitations = sqlx::query_as::<_, Invitation>(
        "select i.id, i.tunnel_id, i.created_by, i.token_hash, i.max_claims, i.claim_count,
                i.created_at, i.expires_at, i.revoked_at
         from invitations i
         join tunnels t on t.id = i.tunnel_id
         where i.tunnel_id = $1 and t.owner_id = $2
         order by i.created_at desc",
    )
    .bind(tunnel_id)
    .bind(owner_id)
    .fetch_all(pool)
    .await?;
    Ok(invitations)
}

/// Record an admission for a guest that just claimed an invitation.
///
/// `source_ip` is parsed and normalised by PostgreSQL's `inet` type rather than being
/// trusted as text: `203.0.113.5` and `::ffff:203.0.113.5` are the same address and would
/// otherwise be admitted twice, defeating the live-set unique index.
///
/// Re-claiming from an address already admitted refreshes the existing row instead of
/// inserting a second one, which is what `data-model.md` §4 requires and what the
/// partial unique index on `(tunnel_id, source_ip) WHERE revoked_at IS NULL` enforces.
pub async fn admit_guest(
    pool: &PgPool,
    invitation_id: Uuid,
    tunnel_id: Uuid,
    source_ip: &str,
    expires_at: DateTime<Utc>,
) -> Result<GuestAdmission> {
    let admission = sqlx::query_as::<_, GuestAdmission>(
        "insert into guest_admissions (invitation_id, tunnel_id, source_ip, expires_at)
         values ($1, $2, $3::inet, $4)
         on conflict (tunnel_id, source_ip) where revoked_at is null
         do update set expires_at = excluded.expires_at
         returning id, invitation_id, tunnel_id, host(source_ip) as source_ip,
                   created_at, expires_at, revoked_at",
    )
    .bind(invitation_id)
    .bind(tunnel_id)
    .bind(source_ip)
    .bind(expires_at)
    .fetch_one(pool)
    .await?;
    Ok(admission)
}

/// The live admission set for a tunnel: what the relay enforces.
///
/// This is the whole table's reason to exist. It is returned to the relay and nowhere
/// else, and it must not be logged on the way past (`PRIV-002`).
pub async fn list_live_admissions(
    pool: &PgPool,
    tunnel_id: Uuid,
    at: DateTime<Utc>,
) -> Result<Vec<GuestAdmission>> {
    let admissions = sqlx::query_as::<_, GuestAdmission>(
        "select id, invitation_id, tunnel_id, host(source_ip) as source_ip,
                created_at, expires_at, revoked_at
         from guest_admissions
         where tunnel_id = $1 and revoked_at is null and expires_at > $2
         order by source_ip",
    )
    .bind(tunnel_id)
    .bind(at)
    .fetch_all(pool)
    .await?;
    Ok(admissions)
}

/// Revoke one admission.
pub async fn revoke_admission(pool: &PgPool, tunnel_id: Uuid, admission_id: Uuid) -> Result<bool> {
    let res = sqlx::query(
        "update guest_admissions set revoked_at = now()
         where id = $1 and tunnel_id = $2 and revoked_at is null",
    )
    .bind(admission_id)
    .bind(tunnel_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Purge admissions past their retention.
///
/// Returns the count so the caller can log the volume without logging any address.
pub async fn purge_expired_admissions(pool: &PgPool, batch: i64) -> Result<u64> {
    let batch = batch.clamp(1, 1000);
    let res = sqlx::query(
        "delete from guest_admissions
         where id in (
             select id from guest_admissions
             where expires_at <= now()
             order by expires_at
             limit $1
         )",
    )
    .bind(batch)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Require a single host address, not a network.
///
/// **Deliberately stricter than the column it guards.** `guest_admissions.source_ip` is a
/// PostgreSQL `inet`, which accepts CIDR notation — `203.0.113.0/24` is a perfectly valid
/// inet value. This function refuses it, and that is the point rather than a gap: an
/// admission grants the relay access to one guest address, so admitting a network would
/// widen one guest's grant into a whole subnet. A caller must not be able to do that by
/// formatting an address differently.
///
/// Two forms of `inet` are refused, and one is forgiven:
///
/// 1. **A CIDR prefix.** A relay that legitimately reports a prefix will be refused here.
///    That is the intended answer for the first cut; if a future relay protocol needs
///    prefixes, the decision belongs in an ADR rather than in this function.
/// 2. **An IPv6 zone id** (`fe80::1%eth0`). Refused because the live-admission unique index
///    is on `(tunnel_id, source_ip)`, so `fe80::1%eth0` and `fe80::1%eth1` would be two rows
///    for one host and would fragment the admission set.
/// 3. **Surrounding whitespace is accepted**, because this value arrives over the wire from
///    a relay and trimming it carries no security meaning. The column stores the normalised
///    form whatever arrives.
///
/// Two further reasons to check here rather than let the driver report it: the caller gets
/// [`Rejection::Field`] instead of a driver error, and **the offending value is not echoed
/// back**. It is guest personal data and error text ends up in logs (PRIV-002).
pub fn assert_valid_source_ip(source_ip: &str) -> Result<()> {
    if source_ip.trim().parse::<std::net::IpAddr>().is_ok() {
        Ok(())
    } else {
        Err(PersistenceError::Rejected(Rejection::Field(
            "source_ip is not a valid IP address",
        )))
    }
}
