//! The application-enforced invariants, as pure functions so they can be tested without
//! a database.
//!
//! | Invariant | Enforced by | Where the check lives |
//! |---|---|---|
//! | INV-1 no local target in any store or log | schema review + test | [`assert_no_local_target_field`], [`scan_for_local_targets`] |
//! | INV-2 mapping key and protocol match the profile | application | [`validate_mapping_against_profile`] |
//! | INV-3 at most one live allocation per tunnel | partial unique index | integration test only |
//! | INV-4 an allocation port's mapping belongs to the allocation's tunnel | application | [`validate_port_mapping_tunnel`] |
//! | INV-5 audit metadata holds no secrets or raw IPs | allow-list + leak test | [`crate::db::audit`] |
//! | INV-6 a public (relay, protocol, port) is held by at most one live mapping | partial unique index | integration test only |
//! | INV-7 terminal tunnel states have `stopped_at` set | CHECK | integration test only |
//!
//! INV-3, INV-6 and INV-7 are database constraints already exercised by the `migrate` CI
//! job; there is nothing for this module to do for them, and pretending otherwise would
//! be a second, weaker copy of a rule the database already owns.

use uuid::Uuid;

use crate::db::entities::Protocol;
use crate::db::profile::ProfileSpec;
use crate::error::{PersistenceError, Rejection, Result};

/// Field names that would mean a host-local target is being stored.
///
/// `_target` and `_destination` are included as separate stems from `_local` because
/// naming a host's destination does not have to use the word "local" — `dest_host` is
/// exactly as much a control-plane-store violation as `local_addr`.
const FORBIDDEN_FIELD_STEMS: &[&str] = &[
    "bind_addr",
    "bind_port",
    "dest_addr",
    "dest_host",
    "dest_port",
    "destination",
    "local_addr",
    "local_host",
    "local_port",
    "local_target",
    "target_addr",
    "target_host",
    "target_port",
];

/// True when `field` is one this layer must refuse to store.
pub fn is_local_target_field(field: &str) -> bool {
    let lowered = field.to_ascii_lowercase();
    FORBIDDEN_FIELD_STEMS
        .iter()
        .any(|stem| lowered.contains(stem))
}

/// INV-1, at the boundary: refuse a field name that would carry a host-local target.
///
/// Called on caller-supplied field names before they reach a query. It is cheap
/// insurance that costs nothing when correct and catches the mistake before a migration
/// makes it permanent, which is the only point at which it is still cheap.
pub fn assert_no_local_target_field(field: &str) -> Result<()> {
    if is_local_target_field(field) {
        return Err(PersistenceError::Rejected(Rejection::LocalTargetField(
            field.to_owned(),
        )));
    }
    Ok(())
}

/// INV-1, over a whole source: report every forbidden field name found.
///
/// Used against the migration DDL and against this crate's own field lists. Returning a
/// list rather than the first hit is deliberate: a test that reports one violation at a
/// time needs several runs to see them all.
pub fn scan_for_local_targets(source: &str) -> Vec<String> {
    let mut found: Vec<String> = FORBIDDEN_FIELD_STEMS
        .iter()
        .filter(|stem| source.to_ascii_lowercase().contains(**stem))
        .map(|stem| (*stem).to_owned())
        .collect();
    found.sort();
    found.dedup();
    found
}

/// INV-2: a mapping's key and protocol must be what the tunnel's profile version says.
///
/// The supplied spec must be the one the tunnel records. Without that check a caller
/// could validate a tunnel's mappings against a different profile and the tunnel would
/// carry mappings its own profile does not define.
pub fn validate_mapping_against_profile(
    mapping_key: &str,
    protocol: Protocol,
    profile: &ProfileSpec,
    tunnel_profile_id: &str,
    tunnel_profile_version: i32,
) -> Result<()> {
    if !profile.matches(tunnel_profile_id, tunnel_profile_version) {
        return Err(PersistenceError::Rejected(Rejection::ProfileSpecMismatch {
            expected_id: tunnel_profile_id.to_owned(),
            expected_version: tunnel_profile_version,
            actual_id: profile.profile_id.clone(),
            actual_version: profile.version,
        }));
    }

    let declared = profile.mapping(mapping_key).ok_or_else(|| {
        PersistenceError::Rejected(Rejection::ProfileMappingUnknown {
            mapping_key: mapping_key.to_owned(),
            profile_id: profile.profile_id.clone(),
            profile_version: profile.version,
        })
    })?;

    if declared.protocol != protocol {
        return Err(PersistenceError::Rejected(
            Rejection::ProfileProtocolMismatch {
                mapping_key: mapping_key.to_owned(),
                expected: declared.protocol.as_str(),
                actual: protocol.as_str(),
            },
        ));
    }
    Ok(())
}

/// INV-4: a port's mapping must belong to the same tunnel as its allocation.
///
/// `allocation_ports` has composite foreign keys to `relay_allocations` and to
/// `tunnel_mappings`, but nothing ties the two to a common tunnel, so the database cannot
/// catch a mapping from tunnel B being published on tunnel A's allocation. That is
/// exactly what `data-model.md` §4 records for this invariant.
pub fn validate_port_mapping_tunnel(
    mapping_id: Uuid,
    mapping_tunnel_id: Uuid,
    allocation_tunnel_id: Uuid,
) -> Result<()> {
    if mapping_tunnel_id != allocation_tunnel_id {
        return Err(PersistenceError::Rejected(
            Rejection::MappingNotInAllocationTunnel {
                mapping_id,
                mapping_tunnel_id,
                allocation_tunnel_id,
            },
        ));
    }
    Ok(())
}
