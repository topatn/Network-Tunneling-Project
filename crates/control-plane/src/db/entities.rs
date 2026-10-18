//! Row types mirroring `migrations/20261018120000_initial_schema.up.sql`.
//!
//! Every field name and type here corresponds to a column. **There is no field for a
//! host-local target address or port, and there must never be one** (INV-1, ADR-003).
//! `tests/invariants_unit.rs` asserts that mechanically against this file, so adding
//! one is a test failure rather than a review question.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Generate a `FromRow` implementation that maps each field to the column of the same
/// name.
///
/// `#[derive(sqlx::FromRow)]` would do this, but it lives behind sqlx's `derive` feature,
/// which pulls in `sqlx-macros` -- a proc-macro crate. A proc macro has to be linked to
/// a dylib, and **this machine cannot link anything**: the GNU toolchain ships no
/// assembler so `dlltool` cannot spawn one, and the MSVC toolchain has no Windows SDK so
/// `link.exe` does not exist. Turning it on would have cost `cargo check`, `cargo clippy`
/// and `cargo test` for this crate in the authoring environment, in exchange for removing
/// this macro. `sqlx::FromRow` the *trait* needs no feature.
///
/// Column names are resolved at runtime by name, exactly as the derive does, so a
/// misspelled field is a runtime error rather than a compile error either way.
macro_rules! impl_from_row {
    ($ty:ty { $($field:ident),* $(,)? }) => {
        impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for $ty {
            // `Result` is fully qualified because a macro_rules! body resolves names
            // at the *expansion* site, where this crate's own `Result` alias is in
            // scope and means something else entirely.
            fn from_row(row: &'r sqlx::postgres::PgRow) -> std::result::Result<Self, sqlx::Error> {
                Ok(Self {
                    $( $field: sqlx::Row::try_get(row, stringify!($field))?, )*
                })
            }
        }
    };
}

/// Re-exported so a query projection can be declared beside the query that builds it,
/// instead of every projection having to live in this module.
pub(crate) use impl_from_row;

/// Wire protocol for a mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// TCP.
    Tcp,
    /// UDP.
    Udp,
}

impl Protocol {
    /// The value stored in `tunnel_mappings.protocol` and `allocation_ports.protocol`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
        }
    }

    /// Parse the stored value. Rejects anything else rather than defaulting, because the
    /// column has a CHECK constraint and a silent default would mask a corrupt row.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "tcp" => Some(Self::Tcp),
            "udp" => Some(Self::Udp),
            _ => None,
        }
    }
}

/// `users.status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserStatus {
    /// Normal state.
    Active,
    /// Suspended by an operator; cannot create or start tunnels or refresh tokens.
    Suspended,
}

impl UserStatus {
    /// The stored value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
        }
    }

    /// Parse the stored value.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "suspended" => Some(Self::Suspended),
            _ => None,
        }
    }
}

/// `tunnels.state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelState {
    /// Created but not started.
    Created,
    /// Running.
    Active,
    /// Stopped by the owner.
    Stopped,
    /// Terminated by an operator or abuse process.
    Terminated,
    /// Reached its expiry.
    Expired,
}

impl TunnelState {
    /// The stored value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Active => "active",
            Self::Stopped => "stopped",
            Self::Terminated => "terminated",
            Self::Expired => "expired",
        }
    }

    /// Parse the stored value.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "created" => Some(Self::Created),
            "active" => Some(Self::Active),
            "stopped" => Some(Self::Stopped),
            "terminated" => Some(Self::Terminated),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }

    /// Whether no further transition is allowed. `data-model.md` §4: terminal states are
    /// final, and a new session is a new row rather than a transition.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Stopped | Self::Terminated | Self::Expired)
    }
}

/// `relay_nodes.status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayStatus {
    /// Being set up.
    Provisioning,
    /// Accepting new allocations.
    Healthy,
    /// Finishing existing allocations, accepting no new ones.
    Draining,
    /// Failing health checks.
    Unhealthy,
    /// Permanently out of service.
    Retired,
}

impl RelayStatus {
    /// The stored value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Provisioning => "provisioning",
            Self::Healthy => "healthy",
            Self::Draining => "draining",
            Self::Unhealthy => "unhealthy",
            Self::Retired => "retired",
        }
    }

    /// Parse the stored value.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "provisioning" => Some(Self::Provisioning),
            "healthy" => Some(Self::Healthy),
            "draining" => Some(Self::Draining),
            "unhealthy" => Some(Self::Unhealthy),
            "retired" => Some(Self::Retired),
            _ => None,
        }
    }

    /// Whether this relay may be given a new allocation.
    pub fn accepts_new_allocations(self) -> bool {
        matches!(self, Self::Healthy)
    }
}

/// `relay_allocations.state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocationState {
    /// Allocated, engine has not connected yet.
    Reserved,
    /// Engine has connected.
    Active,
    /// Handed back.
    Released,
    /// Never used, or past its lifetime.
    Expired,
}

impl AllocationState {
    /// The stored value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::Active => "active",
            Self::Released => "released",
            Self::Expired => "expired",
        }
    }

    /// Parse the stored value.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "reserved" => Some(Self::Reserved),
            "active" => Some(Self::Active),
            "released" => Some(Self::Released),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }

    /// Whether this allocation counts against INV-3's "at most one live allocation".
    pub fn is_live(self) -> bool {
        matches!(self, Self::Reserved | Self::Active)
    }
}

/// `audit_events.actor_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorType {
    /// A `users` row.
    User,
    /// The separate admin identity (ADR-019), never a role on `users`.
    Admin,
    /// An automated process with no user behind it.
    System,
    /// A relay reporting something.
    Relay,
}

impl ActorType {
    /// The stored value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Admin => "admin",
            Self::System => "system",
            Self::Relay => "relay",
        }
    }

    /// Parse the stored value.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "admin" => Some(Self::Admin),
            "system" => Some(Self::System),
            "relay" => Some(Self::Relay),
            _ => None,
        }
    }
}

/// A row of `users`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    /// Primary key.
    pub id: Uuid,
    /// Identity provider issuer. Not secret.
    pub idp_issuer: String,
    /// Identity provider subject. **Sensitive** (`data-model.md` §4).
    pub idp_subject: String,
    /// Optional display name, 1-64 characters.
    pub display_name: Option<String>,
    /// Account state.
    pub status: String,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Soft-deletion time.
    pub deleted_at: Option<DateTime<Utc>>,
}

impl_from_row!(User {
    id,
    idp_issuer,
    idp_subject,
    display_name,
    status,
    created_at,
    deleted_at
});

/// A row of `auth_sessions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSession {
    /// Primary key.
    pub id: Uuid,
    /// Owning user.
    pub user_id: Uuid,
    /// Rotation family; presenting a used token revokes the whole family.
    pub family_id: Uuid,
    /// Hash of the refresh token, never the token. **Sensitive.**
    pub refresh_token_hash: Vec<u8>,
    /// Optional device label, at most 64 characters.
    pub device_label: Option<String>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last use, for `contracts.md` §9 absolute-lifetime enforcement.
    pub last_used_at: Option<DateTime<Utc>>,
    /// Expiry, strictly after `created_at`.
    pub expires_at: DateTime<Utc>,
    /// Revocation time.
    pub revoked_at: Option<DateTime<Utc>>,
}

impl_from_row!(AuthSession {
    id,
    user_id,
    family_id,
    refresh_token_hash,
    device_label,
    created_at,
    last_used_at,
    expires_at,
    revoked_at
});

/// A row of `tunnels`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tunnel {
    /// Primary key.
    pub id: Uuid,
    /// Owner. Every owner-scoped query filters on this (SEC-009).
    pub owner_id: Uuid,
    /// User-facing name, 1-64 characters.
    pub name: String,
    /// Profile this tunnel was created against.
    pub game_profile_id: String,
    /// Version of that profile.
    pub profile_version: i32,
    /// Lifecycle state.
    pub state: String,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Start time.
    pub started_at: Option<DateTime<Utc>>,
    /// Stop time; non-null exactly when the state is terminal (INV-7).
    pub stopped_at: Option<DateTime<Utc>>,
    /// Expiry, strictly after `created_at`.
    pub expires_at: DateTime<Utc>,
    /// Why a terminal tunnel ended.
    pub termination_reason: Option<String>,
}

impl_from_row!(Tunnel {
    id,
    owner_id,
    name,
    game_profile_id,
    profile_version,
    state,
    created_at,
    started_at,
    stopped_at,
    expires_at,
    termination_reason
});

/// A row of `tunnel_mappings`.
///
/// Deliberately has **no** local address or port field (INV-1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelMapping {
    /// Primary key.
    pub id: Uuid,
    /// Owning tunnel.
    pub tunnel_id: Uuid,
    /// Profile-defined key.
    pub mapping_key: String,
    /// Protocol, matching the profile (INV-2).
    pub protocol: String,
    /// Index used in UDP datagram headers (`contracts.md` §11).
    pub position: i16,
    /// Whether the mapping is active.
    pub enabled: bool,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

impl_from_row!(TunnelMapping {
    id,
    tunnel_id,
    mapping_key,
    protocol,
    position,
    enabled,
    created_at
});

/// A row of `relay_nodes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayNode {
    /// Primary key.
    pub id: Uuid,
    /// Unique operator-chosen name.
    pub name: String,
    /// Region label.
    pub region: String,
    /// The relay's own public hostname. This is the **relay's** address, not a
    /// host-local target, which is the distinction INV-1 draws.
    pub public_host: String,
    /// Lifecycle state.
    pub status: String,
    /// Maximum concurrent allocations.
    pub max_allocations: i32,
    /// First port the relay may hand out.
    pub port_range_start: i32,
    /// Last port the relay may hand out.
    pub port_range_end: i32,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last health report.
    pub last_health_at: Option<DateTime<Utc>>,
}

impl_from_row!(RelayNode {
    id,
    name,
    region,
    public_host,
    status,
    max_allocations,
    port_range_start,
    port_range_end,
    created_at,
    last_health_at
});

/// A row of `relay_allocations`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayAllocation {
    /// Primary key.
    pub id: Uuid,
    /// Tunnel the allocation is for.
    pub tunnel_id: Uuid,
    /// Relay hosting it.
    pub relay_id: Uuid,
    /// Lifecycle state.
    pub state: String,
    /// Issue time.
    pub issued_at: DateTime<Utc>,
    /// Expiry, strictly after `issued_at`.
    pub expires_at: DateTime<Utc>,
    /// Release time.
    pub released_at: Option<DateTime<Utc>>,
}

impl_from_row!(RelayAllocation {
    id,
    tunnel_id,
    relay_id,
    state,
    issued_at,
    expires_at,
    released_at
});

/// A row of `allocation_ports`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllocationPort {
    /// Allocation this port belongs to.
    pub allocation_id: Uuid,
    /// Mapping this port publishes.
    pub mapping_id: Uuid,
    /// Relay holding the port.
    pub relay_id: Uuid,
    /// Protocol of the mapping.
    pub protocol: String,
    /// The relay's public port. **Not** a host-local port (INV-1).
    pub public_port: i32,
    /// Release time.
    pub released_at: Option<DateTime<Utc>>,
}

impl_from_row!(AllocationPort {
    allocation_id,
    mapping_id,
    relay_id,
    protocol,
    public_port,
    released_at
});

/// A row of `invitations`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invitation {
    /// Primary key.
    pub id: Uuid,
    /// Tunnel the invitation grants access to.
    pub tunnel_id: Uuid,
    /// User who created it.
    pub created_by: Uuid,
    /// Hash of the invitation secret, never the secret. **Sensitive.**
    pub token_hash: Vec<u8>,
    /// Claim ceiling, 1-50.
    pub max_claims: i32,
    /// Claims so far, never above `max_claims`.
    pub claim_count: i32,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Expiry, strictly after `created_at`.
    pub expires_at: DateTime<Utc>,
    /// Revocation time.
    pub revoked_at: Option<DateTime<Utc>>,
}

impl_from_row!(Invitation {
    id,
    tunnel_id,
    created_by,
    token_hash,
    max_claims,
    claim_count,
    created_at,
    expires_at,
    revoked_at
});

/// A row of `guest_admissions`.
///
/// `source_ip` is personal data. It exists here because the relay enforces the live set
/// (E-13), and it must never reach a log or audit metadata (INV-5, PRIV-002).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestAdmission {
    /// Primary key.
    pub id: Uuid,
    /// Invitation that produced this admission.
    pub invitation_id: Uuid,
    /// Tunnel admitted, denormalised for the composite foreign key.
    pub tunnel_id: Uuid,
    /// The guest's address as the relay saw it.
    pub source_ip: String,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Expiry, strictly after `created_at`.
    pub expires_at: DateTime<Utc>,
    /// Revocation time.
    pub revoked_at: Option<DateTime<Utc>>,
}

impl_from_row!(GuestAdmission {
    id,
    invitation_id,
    tunnel_id,
    source_ip,
    created_at,
    expires_at,
    revoked_at
});

/// A row of `usage_records`, already aggregated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRecord {
    /// Tunnel measured.
    pub tunnel_id: Uuid,
    /// Relay that carried the traffic.
    pub relay_id: Uuid,
    /// Start of the interval.
    pub interval_start: DateTime<Utc>,
    /// Bytes sent to guests.
    pub bytes_from_guests: i64,
    /// Bytes received from guests.
    pub bytes_to_guests: i64,
    /// Peak concurrent flows.
    pub peak_flows: i32,
    /// Datagrams the relay dropped.
    pub datagrams_dropped: i64,
}

impl_from_row!(UsageRecord {
    tunnel_id,
    relay_id,
    interval_start,
    bytes_from_guests,
    bytes_to_guests,
    peak_flows,
    datagrams_dropped
});

/// A row of `audit_events`. Append-only: never updated, never deleted while retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEvent {
    /// Identity primary key.
    pub id: i64,
    /// When it happened.
    pub occurred_at: DateTime<Utc>,
    /// Who did it.
    pub actor_type: String,
    /// Which identity, when there is one.
    pub actor_id: Option<Uuid>,
    /// What happened. Vocabulary is not yet enumerated in `contracts.md`.
    pub action: String,
    /// What it happened to.
    pub subject_type: String,
    /// Which instance of that subject.
    pub subject_id: Option<Uuid>,
    /// Correlates the writes of one request.
    pub request_id: Option<String>,
    /// Validated by INV-5 before it got here.
    pub metadata: serde_json::Value,
}

impl_from_row!(AuditEvent {
    id,
    occurred_at,
    actor_type,
    actor_id,
    action,
    subject_type,
    subject_id,
    request_id,
    metadata
});

/// A row of `idempotency_keys`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyRecord {
    /// Scoping user.
    pub user_id: Uuid,
    /// Route the key was used on.
    pub route: String,
    /// The client-supplied key, 8-128 characters.
    pub idem_key: String,
    /// Hash of the request body, used to detect a key reused with a different body.
    pub request_hash: Vec<u8>,
    /// Stored response status, once a response exists.
    pub response_status: Option<i32>,
    /// Stored response body, once a response exists.
    pub response_body: Option<serde_json::Value>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Expiry after which the key may be forgotten.
    pub expires_at: DateTime<Utc>,
}

impl_from_row!(IdempotencyRecord {
    user_id,
    route,
    idem_key,
    request_hash,
    response_status,
    response_body,
    created_at,
    expires_at
});
