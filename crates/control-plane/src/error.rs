//! Error type for the persistence layer.
//!
//! Hand-rolled rather than pulled from `thiserror`: `AGENTS.md` §5.2 says not to add a
//! dependency for a few lines of code, and this enum plus its `Display` impl is that.

use std::error::Error;
use std::fmt;

/// Why a value was refused before it reached the database.
///
/// Each variant names the invariant it protects, because the caller needs to map this to
/// the right `contracts.md` §3 error code and guessing is how `forbidden` ends up
/// returned where `not_found` is required.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// INV-1. A field name that would carry a host-local target reached this layer.
    /// Nothing legitimate in the control plane has such a name; the check exists so the
    /// mistake is caught at the boundary rather than after a migration ships it.
    LocalTargetField(String),
    /// INV-2. The mapping's protocol disagrees with the profile's for that key.
    ProfileProtocolMismatch {
        /// The `mapping_key` that was checked.
        mapping_key: String,
        /// Protocol the profile declares for that key.
        expected: &'static str,
        /// Protocol the caller asked for.
        actual: &'static str,
    },
    /// INV-2. The mapping key is not declared by this profile version at all.
    ProfileMappingUnknown {
        /// The `mapping_key` that was not found.
        mapping_key: String,
        /// Profile the lookup was against.
        profile_id: String,
        /// Version the lookup was against.
        profile_version: i32,
    },
    /// INV-2. The tunnel records one `(profile_id, version)` and the supplied profile is
    /// a different one. Catching it here stops a tunnel silently validating against a
    /// profile it was never created with.
    ProfileSpecMismatch {
        /// Profile recorded on the tunnel.
        expected_id: String,
        /// Version recorded on the tunnel.
        expected_version: i32,
        /// Profile that was supplied.
        actual_id: String,
        /// Version that was supplied.
        actual_version: i32,
    },
    /// INV-4. The mapping named for an allocation port belongs to a different tunnel than
    /// the allocation does. The composite foreign keys do not catch this, which is
    /// precisely why it is checked here.
    MappingNotInAllocationTunnel {
        /// Mapping that was named.
        mapping_id: uuid::Uuid,
        /// Tunnel the mapping actually belongs to.
        mapping_tunnel_id: uuid::Uuid,
        /// Tunnel the allocation belongs to.
        allocation_tunnel_id: uuid::Uuid,
    },
    /// INV-5. Audit metadata was refused. `reason` is a short fixed phrase, never the
    /// offending value, so a rejection cannot become a leak path (`AGENTS.md` §7).
    AuditMetadata(&'static str),
    /// A value failed a bounded validation that is not one of the named invariants.
    Field(&'static str),
}

/// Everything the persistence layer can fail with.
#[derive(Debug)]
pub enum PersistenceError {
    /// The database rejected a statement, or the connection failed.
    ///
    /// Every database call in this crate runs inside a transaction or a single
    /// statement, so this is the only place driver errors surface to callers.
    Database(sqlx::Error),
    /// An application-level rule refused the value.
    Rejected(Rejection),
}

impl PersistenceError {
    /// The rejection reason, if this is an application-level refusal.
    pub fn rejection(&self) -> Option<&Rejection> {
        match self {
            Self::Rejected(r) => Some(r),
            Self::Database(_) => None,
        }
    }
}

impl fmt::Display for PersistenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // The driver's own message can contain a bound value, so it is not
            // interpolated into anything that looks like a log line. Callers that want
            // detail log the `sqlx::Error` themselves, where the redaction rules of
            // PRIV-002 apply deliberately rather than by accident.
            Self::Database(_) => f.write_str("database error"),
            Self::Rejected(r) => match r {
                Rejection::LocalTargetField(name) => {
                    write!(f, "field {name:?} would store a host-local target (INV-1)")
                }
                Rejection::ProfileProtocolMismatch {
                    mapping_key,
                    expected,
                    actual,
                } => write!(
                    f,
                    "mapping {mapping_key:?} is {expected} in this profile, not {actual} (INV-2)"
                ),
                Rejection::ProfileMappingUnknown {
                    mapping_key,
                    profile_id,
                    profile_version,
                } => write!(
                    f,
                    "profile {profile_id} v{profile_version} does not declare mapping \
                     {mapping_key:?} (INV-2)"
                ),
                Rejection::ProfileSpecMismatch {
                    expected_id,
                    expected_version,
                    actual_id,
                    actual_version,
                } => write!(
                    f,
                    "tunnel records profile {expected_id} v{expected_version}, but \
                     {actual_id} v{actual_version} was supplied (INV-2)"
                ),
                Rejection::MappingNotInAllocationTunnel {
                    mapping_tunnel_id,
                    allocation_tunnel_id,
                    ..
                } => write!(
                    f,
                    "mapping belongs to tunnel {mapping_tunnel_id} but the allocation \
                     belongs to {allocation_tunnel_id} (INV-4)"
                ),
                Rejection::AuditMetadata(reason) => {
                    write!(f, "audit metadata rejected: {reason} (INV-5)")
                }
                Rejection::Field(name) => write!(f, "field {name:?} failed validation"),
            },
        }
    }
}

impl Error for PersistenceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Database(e) => Some(e),
            Self::Rejected(_) => None,
        }
    }
}

impl From<sqlx::Error> for PersistenceError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e)
    }
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, PersistenceError>;
