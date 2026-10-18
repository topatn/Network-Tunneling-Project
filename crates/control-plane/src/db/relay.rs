//! `relay_nodes`, `relay_allocations`, and `allocation_ports`.
//!
//! INV-3 and INV-6 are enforced here and by partial unique indexes on the same
//! statements: the index is what actually guarantees them under concurrency, and the
//! pre-checks exist to turn a violation into a clear error instead of a raw constraint
//! name. **The pre-checks are not the enforcement** — two concurrent requests can both
//! pass a pre-check, and only the index makes one of them fail.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::entities::impl_from_row;
use crate::db::entities::{AllocationPort, Protocol, RelayAllocation, RelayNode, RelayStatus};
use crate::db::invariants::validate_port_mapping_tunnel;
use crate::error::{PersistenceError, Rejection, Result};

/// Register a relay node. Operator-managed, so there is no owner scoping.
pub async fn create_relay_node(
    pool: &PgPool,
    name: &str,
    region: &str,
    public_host: &str,
    max_allocations: i32,
    port_range_start: i32,
    port_range_end: i32,
) -> Result<RelayNode> {
    let node = sqlx::query_as::<_, RelayNode>(
        "insert into relay_nodes
             (name, region, public_host, max_allocations, port_range_start, port_range_end)
         values ($1, $2, $3, $4, $5, $6)
         returning id, name, region, public_host, status, max_allocations,
                   port_range_start, port_range_end, created_at, last_health_at",
    )
    .bind(name)
    .bind(region)
    .bind(public_host)
    .bind(max_allocations)
    .bind(port_range_start)
    .bind(port_range_end)
    .fetch_one(pool)
    .await?;
    Ok(node)
}

/// Fetch a relay node.
pub async fn get_relay_node(pool: &PgPool, relay_id: Uuid) -> Result<Option<RelayNode>> {
    let node = sqlx::query_as::<_, RelayNode>(
        "select id, name, region, public_host, status, max_allocations,
                port_range_start, port_range_end, created_at, last_health_at
         from relay_nodes where id = $1",
    )
    .bind(relay_id)
    .fetch_optional(pool)
    .await?;
    Ok(node)
}

/// List relays eligible to receive a new allocation.
///
/// `accepts_new_allocations` is applied in SQL rather than in Rust so the list cannot be
/// wrong because a row was fetched and then filtered. Bounded for the same reason as
/// every other list here.
pub async fn list_allocatable_relays(pool: &PgPool, limit: i64) -> Result<Vec<RelayNode>> {
    let limit = limit.clamp(1, 200);
    let nodes = sqlx::query_as::<_, RelayNode>(
        "select id, name, region, public_host, status, max_allocations,
                port_range_start, port_range_end, created_at, last_health_at
         from relay_nodes
         where status = 'healthy'
         order by name
         limit $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(nodes)
}

/// Change a relay's status and record a health observation.
pub async fn set_relay_status(
    pool: &PgPool,
    relay_id: Uuid,
    status: RelayStatus,
    healthy: bool,
) -> Result<bool> {
    let res = sqlx::query(
        "update relay_nodes
         set status = $2,
             last_health_at = case when $3 then now() else last_health_at end
         where id = $1",
    )
    .bind(relay_id)
    .bind(status.as_str())
    .bind(healthy)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Reserve an allocation for a tunnel. INV-3 applies here.
///
/// The live-allocation partial unique index is the real enforcement. The pre-check gives a
/// caller a comprehensible error instead of a database constraint name, and the index
/// still decides the outcome when two requests race.
pub async fn reserve_allocation(
    pool: &PgPool,
    tunnel_id: Uuid,
    relay_id: Uuid,
    expires_at: DateTime<Utc>,
) -> Result<RelayAllocation> {
    let existing = sqlx::query_scalar::<_, i64>(
        "select count(*) from relay_allocations
         where tunnel_id = $1 and state in ('reserved', 'active')",
    )
    .bind(tunnel_id)
    .fetch_one(pool)
    .await?;
    if existing > 0 {
        return Err(PersistenceError::Rejected(Rejection::Field(
            "tunnel already has a live allocation (INV-3)",
        )));
    }

    let allocation = sqlx::query_as::<_, RelayAllocation>(
        "insert into relay_allocations (tunnel_id, relay_id, expires_at)
         values ($1, $2, $3)
         returning id, tunnel_id, relay_id, state, issued_at, expires_at, released_at",
    )
    .bind(tunnel_id)
    .bind(relay_id)
    .bind(expires_at)
    .fetch_one(pool)
    .await?;
    Ok(allocation)
}

/// Mark a reserved allocation active, once the engine has connected.
pub async fn activate_allocation(
    pool: &PgPool,
    allocation_id: Uuid,
    relay_id: Uuid,
) -> Result<bool> {
    let res = sqlx::query(
        "update relay_allocations set state = 'active'
         where id = $1 and relay_id = $2 and state = 'reserved'",
    )
    .bind(allocation_id)
    .bind(relay_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Release an allocation and free its ports.
///
/// One transaction, because a released allocation whose ports still look live would keep
/// the port occupied under INV-6 while nothing can reach it.
pub async fn release_allocation(pool: &PgPool, allocation_id: Uuid) -> Result<bool> {
    let mut tx = pool.begin().await?;

    let res = sqlx::query(
        "update relay_allocations set state = 'released', released_at = now()
         where id = $1 and state in ('reserved', 'active')",
    )
    .bind(allocation_id)
    .execute(&mut *tx)
    .await?;
    if res.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(false);
    }

    sqlx::query(
        "update allocation_ports set released_at = now()
         where allocation_id = $1 and released_at is null",
    )
    .bind(allocation_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(true)
}

/// Expire allocations that were reserved but never used, or have outlived their tunnel.
pub async fn expire_stale_allocations(pool: &PgPool, batch: i64) -> Result<u64> {
    let batch = batch.clamp(1, 200);
    let mut tx = pool.begin().await?;
    let res = sqlx::query(
        "update relay_allocations set state = 'expired', released_at = now()
         where id in (
             select id from relay_allocations
             where state = 'reserved' and expires_at <= now()
             order by expires_at
             limit $1
             for update skip locked
         )",
    )
    .bind(batch)
    .execute(&mut *tx)
    .await?;
    let n = res.rows_affected();

    if n > 0 {
        sqlx::query(
            "update allocation_ports set released_at = now()
             where released_at is null
               and allocation_id in (select id from relay_allocations where state = 'expired')",
        )
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(n)
}

/// The tunnel's live allocation, if it has one.
pub async fn get_live_allocation(
    pool: &PgPool,
    tunnel_id: Uuid,
) -> Result<Option<RelayAllocation>> {
    let allocation = sqlx::query_as::<_, RelayAllocation>(
        "select id, tunnel_id, relay_id, state, issued_at, expires_at, released_at
         from relay_allocations
         where tunnel_id = $1 and state in ('reserved', 'active')",
    )
    .bind(tunnel_id)
    .fetch_optional(pool)
    .await?;
    Ok(allocation)
}

/// An allocation together with the tunnel that owns it.
///
/// A named projection rather than a tuple: a tuple of structs goes through `Decode`,
/// which `(A, B)` does not implement, so a tuple here simply does not compile. Flat, like
/// every other projection, because a projection's job is to mirror the selected columns.
///
/// The owner id is included so a caller can scope what it does next, rather than fetching
/// the tunnel separately and creating a window in which ownership could change between
/// the two reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllocationWithOwner {
    /// Allocation primary key.
    pub id: Uuid,
    /// Tunnel the allocation is for.
    pub tunnel_id: Uuid,
    /// Relay hosting it.
    pub relay_id: Uuid,
    /// Allocation state.
    pub state: String,
    /// Issue time.
    pub issued_at: DateTime<Utc>,
    /// Expiry.
    pub expires_at: DateTime<Utc>,
    /// Release time.
    pub released_at: Option<DateTime<Utc>>,
    /// Owner of the tunnel, joined in.
    pub owner_id: Uuid,
}

impl_from_row!(AllocationWithOwner {
    id,
    tunnel_id,
    relay_id,
    state,
    issued_at,
    expires_at,
    released_at,
    owner_id,
});

impl AllocationWithOwner {
    /// The allocation on its own, for a caller that does not need the joined owner.
    pub fn allocation(&self) -> RelayAllocation {
        RelayAllocation {
            id: self.id,
            tunnel_id: self.tunnel_id,
            relay_id: self.relay_id,
            state: self.state.clone(),
            issued_at: self.issued_at,
            expires_at: self.expires_at,
            released_at: self.released_at,
        }
    }
}

/// Fetch an allocation with the tunnel it belongs to, which is what INV-4 needs.
pub async fn get_allocation_with_tunnel(
    pool: &PgPool,
    allocation_id: Uuid,
) -> Result<Option<AllocationWithOwner>> {
    let row = sqlx::query_as::<_, AllocationWithOwner>(
        "select a.id, a.tunnel_id, a.relay_id, a.state, a.issued_at, a.expires_at,
                a.released_at, t.owner_id
         from relay_allocations a
         join tunnels t on t.id = a.tunnel_id
         where a.id = $1",
    )
    .bind(allocation_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Publish a port for a mapping on an allocation.
///
/// Enforces **INV-4** (the mapping must belong to the allocation's tunnel) and lets the
/// database enforce **INV-6** (no two live mappings share a public port).
///
/// Both mappings and the allocation are read inside the transaction, and the read of the
/// mapping's tunnel is what INV-4 is checked against. Reading them outside the
/// transaction would leave a window in which the mapping's tunnel changed between the
/// check and the insert; the composite foreign keys stop the row being wrong, but not
/// the check being stale.
pub async fn publish_allocation_port(
    pool: &PgPool,
    allocation_id: Uuid,
    mapping_id: Uuid,
    relay_id: Uuid,
    protocol: Protocol,
    public_port: i32,
) -> Result<AllocationPort> {
    let mut tx = pool.begin().await?;

    let allocation_tunnel = sqlx::query_scalar::<_, Uuid>(
        "select tunnel_id from relay_allocations where id = $1 and relay_id = $2 for share",
    )
    .bind(allocation_id)
    .bind(relay_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(allocation_tunnel) = allocation_tunnel else {
        tx.rollback().await?;
        return Err(PersistenceError::Rejected(Rejection::Field(
            "no such allocation on that relay",
        )));
    };

    let mapping_tunnel = sqlx::query_scalar::<_, Uuid>(
        "select tunnel_id from tunnel_mappings where id = $1 for share",
    )
    .bind(mapping_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(mapping_tunnel) = mapping_tunnel else {
        tx.rollback().await?;
        return Err(PersistenceError::Rejected(Rejection::Field(
            "no such mapping",
        )));
    };

    validate_port_mapping_tunnel(mapping_id, mapping_tunnel, allocation_tunnel)?;

    let port = sqlx::query_as::<_, AllocationPort>(
        "insert into allocation_ports (allocation_id, mapping_id, relay_id, protocol, public_port)
         values ($1, $2, $3, $4, $5)
         returning allocation_id, mapping_id, relay_id, protocol, public_port, released_at",
    )
    .bind(allocation_id)
    .bind(mapping_id)
    .bind(relay_id)
    .bind(protocol.as_str())
    .bind(public_port)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(port)
}

/// List the ports published by an allocation, in mapping order.
pub async fn list_allocation_ports(
    pool: &PgPool,
    allocation_id: Uuid,
) -> Result<Vec<AllocationPort>> {
    let ports = sqlx::query_as::<_, AllocationPort>(
        "select allocation_id, mapping_id, relay_id, protocol, public_port, released_at
         from allocation_ports
         where allocation_id = $1
         order by mapping_id",
    )
    .bind(allocation_id)
    .fetch_all(pool)
    .await?;
    Ok(ports)
}

/// Release a single published port.
pub async fn release_allocation_port(
    pool: &PgPool,
    allocation_id: Uuid,
    mapping_id: Uuid,
) -> Result<bool> {
    let res = sqlx::query(
        "update allocation_ports set released_at = now()
         where allocation_id = $1 and mapping_id = $2 and released_at is null",
    )
    .bind(allocation_id)
    .bind(mapping_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Relays ordered by how many live allocations they carry, for capacity decisions.
pub async fn relay_load(pool: &PgPool) -> Result<Vec<(Uuid, i64, i32)>> {
    let rows = sqlx::query_as::<_, (Uuid, i64, i32)>(
        "select n.id,
                count(a.id) filter (where a.state in ('reserved', 'active')) as live,
                n.max_allocations
         from relay_nodes n
         left join relay_allocations a on a.relay_id = n.id
         group by n.id, n.max_allocations
         order by live desc, n.name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
