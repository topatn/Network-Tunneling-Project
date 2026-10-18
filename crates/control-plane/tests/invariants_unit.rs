//! Invariant tests that need no database: INV-1, INV-2, INV-4 and INV-5.
//!
//! These run on every machine, including the authoring environment where no normal
//! PostgreSQL client connection is possible (`schema.md`). That split is deliberate: the
//! application-enforced invariants are pure functions, so testing them does not require
//! the one thing this project currently cannot do locally.
//!
//! Each invariant has both a passing case and a failing-input case, because T-034's
//! Done-when condition requires both, and because a test that only proves the happy path
//! would pass just as well if the check were deleted.
//!
//! No `unwrap`, `expect` or `panic!`: the workspace denies the `panic` lint group on all
//! targets, so these return `Result` and use `?`.

use gtt_control_plane::db::audit;
use gtt_control_plane::db::entities::Protocol;
use gtt_control_plane::db::invariants::{
    assert_no_local_target_field, is_local_target_field, scan_for_local_targets,
    validate_mapping_against_profile, validate_port_mapping_tunnel,
};
use gtt_control_plane::db::profile::{ProfileMapping, ProfileSpec};
use gtt_control_plane::db::sharing;
use gtt_control_plane::error::Rejection;
use uuid::Uuid;

/// The entity definitions, compiled in rather than read at runtime, so this test cannot
/// pass by failing to find the file.
const ENTITIES_SRC: &str = include_str!("../src/db/entities.rs");
// Three levels up, not two: this file is `crates/control-plane/tests/`, so
// `../../` is `crates/`. The migration is at the repository root.
const MIGRATION_SRC: &str =
    include_str!("../../../migrations/20261018120000_initial_schema.up.sql");

fn test_profile() -> ProfileSpec {
    ProfileSpec {
        profile_id: "minecraft-java".to_owned(),
        version: 1,
        mappings: vec![
            ProfileMapping {
                mapping_key: "query".to_owned(),
                protocol: Protocol::Udp,
                enabled_by_default: true,
            },
            ProfileMapping {
                mapping_key: "rccon".to_owned(),
                protocol: Protocol::Tcp,
                enabled_by_default: false,
            },
        ],
    }
}

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Pull the application-level rejection out of a failed call.
///
/// Returns `Err` with a description instead of panicking, because the workspace denies the
/// `panic` lint group on every target including tests, and `assert!(false, ..)` is caught
/// by `assertions_on_constants` in the denied `style` group. A test that returns `Err`
/// reports the same message without any of that.
fn rejection_of(
    outcome: std::result::Result<(), gtt_control_plane::PersistenceError>,
) -> std::result::Result<Rejection, String> {
    match outcome {
        Ok(()) => Err("expected a rejection, but the value was accepted".to_owned()),
        Err(e) => match e.rejection().cloned() {
            Some(r) => Ok(r),
            None => Err(format!("expected an application rejection, got: {e}")),
        },
    }
}

// ---------------------------------------------------------------------------
// INV-1 -- no host-local target address or port in any control-plane store
// ---------------------------------------------------------------------------

#[test]
fn inv1_no_local_target_field_in_the_schema() {
    // Passing case: the applied schema holds nothing that names a host-local target.
    assert_eq!(
        scan_for_local_targets(MIGRATION_SRC),
        Vec::<String>::new(),
        "the migration names a host-local target field, which INV-1 forbids"
    );
}

#[test]
fn inv1_no_local_target_field_in_the_entity_types() {
    // Passing case: no Rust field in this crate's row types names one either. The schema
    // is not the only place a target could be recorded, so both are scanned.
    assert_eq!(
        scan_for_local_targets(ENTITIES_SRC),
        Vec::<String>::new(),
        "an entity type names a host-local target field, which INV-1 forbids"
    );
}

#[test]
fn inv1_rejects_a_local_target_field_name() -> TestResult {
    // Failing-input case: the boundary check refuses each shape a target could take.
    for field in [
        "local_addr",
        "local_port",
        "target_host",
        "dest_port",
        "destination",
        "bind_addr",
    ] {
        let outcome = assert_no_local_target_field(field);
        assert!(
            outcome.is_err(),
            "{field:?} should have been refused by INV-1"
        );
        let rejection = rejection_of(outcome)?;
        assert!(
            matches!(rejection, Rejection::LocalTargetField(_)),
            "{field:?} should be refused as a local-target field, got {rejection:?}"
        );
    }

    // And a legitimate field name passes, so the check is not simply refusing everything.
    for field in [
        "relay_nodes.public_host",
        "allocation_ports.public_port",
        "source_ip",
    ] {
        assert!(
            assert_no_local_target_field(field).is_ok(),
            "{field:?} is not a host-local target and must be accepted"
        );
    }
    Ok(())
}

#[test]
fn inv1_scan_reports_a_planted_target() {
    // Failing-input case for the scanner itself: a planted target is found, so a clean
    // result elsewhere is a real result and not a scanner that never matches.
    let planted = "create table x (local_addr inet not null, public_port integer)";
    let found = scan_for_local_targets(planted);
    assert!(
        found.contains(&"local_addr".to_owned()),
        "the scanner missed a planted local target; found {found:?}"
    );

    // A word that merely contains a forbidden stem must not trip it, or the check would
    // be noise nobody could act on.
    assert!(
        !is_local_target_field("catalog_target_count"),
        "the stem check should not fire on an unrelated word containing 'target'"
    );
}

// ---------------------------------------------------------------------------
// INV-2 -- mapping key and protocol match the tunnel's profile version
// ---------------------------------------------------------------------------

#[test]
fn inv2_accepts_a_mapping_the_profile_declares() -> TestResult {
    // Passing cases: each mapping the profile declares is accepted at its declared
    // protocol, and an optional mapping that is off by default is still valid.
    let profile = test_profile();
    for (key, protocol) in [("query", Protocol::Udp), ("rccon", Protocol::Tcp)] {
        validate_mapping_against_profile(key, protocol, &profile, "minecraft-java", 1)?;
    }
    assert!(
        !profile
            .mapping("rccon")
            .map(|m| m.enabled_by_default)
            .unwrap_or(true),
        "rccon is an optional mapping and must default to off (GAME-008)"
    );
    Ok(())
}

#[test]
fn inv2_rejects_a_protocol_the_profile_does_not_declare() -> TestResult {
    // Failing-input case: `query` is UDP in this profile, so asking for TCP must fail.
    let profile = test_profile();
    let outcome =
        validate_mapping_against_profile("query", Protocol::Tcp, &profile, "minecraft-java", 1);

    let rejection = rejection_of(outcome)?;
    let Rejection::ProfileProtocolMismatch {
        expected, actual, ..
    } = rejection
    else {
        return Err(format!("expected a protocol mismatch, got {rejection:?}").into());
    };
    assert_eq!(
        expected, "udp",
        "the expected protocol should come from the profile"
    );
    assert_eq!(
        actual, "tcp",
        "the actual protocol should be what was asked for"
    );
    Ok(())
}

#[test]
fn inv2_rejects_an_undeclared_mapping_key() -> TestResult {
    // Failing-input case: a key no version of this profile defines.
    let profile = test_profile();
    let outcome =
        validate_mapping_against_profile("bedrock", Protocol::Udp, &profile, "minecraft-java", 1);

    let rejection = rejection_of(outcome)?;
    let Rejection::ProfileMappingUnknown {
        profile_version, ..
    } = rejection
    else {
        return Err(format!("expected an unknown-mapping rejection, got {rejection:?}").into());
    };
    assert_eq!(profile_version, 1);
    Ok(())
}

#[test]
fn inv2_rejects_a_spec_for_a_different_profile_version() -> TestResult {
    // Failing-input case: validating against a profile the tunnel was not created with.
    // Without this, a caller could check a tunnel's mappings against a v2 profile and the
    // tunnel would end up carrying v1 mappings that passed a v2 check.
    let mut v2 = test_profile();
    v2.version = 2;
    let outcome =
        validate_mapping_against_profile("query", Protocol::Udp, &v2, "minecraft-java", 1);

    let rejection = rejection_of(outcome)?;
    let Rejection::ProfileSpecMismatch {
        expected_version,
        actual_version,
        ..
    } = rejection
    else {
        return Err(format!("expected a profile-spec mismatch, got {rejection:?}").into());
    };
    assert_eq!(
        expected_version, 1,
        "the tunnel's version is what must be matched"
    );
    assert_eq!(
        actual_version, 2,
        "the supplied spec's version is the mismatch"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// INV-4 -- an allocation port's mapping belongs to the allocation's tunnel
// ---------------------------------------------------------------------------

#[test]
fn inv4_accepts_a_mapping_from_the_allocation_tunnel() -> TestResult {
    let tunnel = Uuid::new_v4();
    let mapping = Uuid::new_v4();
    validate_port_mapping_tunnel(mapping, tunnel, tunnel)?;
    Ok(())
}

#[test]
fn inv4_rejects_a_mapping_from_another_tunnel() -> TestResult {
    // Failing-input case. The composite foreign keys in `allocation_ports` cannot catch
    // this: they tie the port to an allocation and to a mapping independently, and
    // nothing requires the two to belong to the same tunnel.
    let allocation_tunnel = Uuid::new_v4();
    let other_tunnel = Uuid::new_v4();
    let mapping = Uuid::new_v4();

    let outcome = validate_port_mapping_tunnel(mapping, other_tunnel, allocation_tunnel);

    let rejection = rejection_of(outcome)?;
    let Rejection::MappingNotInAllocationTunnel {
        mapping_tunnel_id,
        allocation_tunnel_id,
        ..
    } = rejection
    else {
        return Err(format!("expected a cross-tunnel mapping rejection, got {rejection:?}").into());
    };
    assert_eq!(mapping_tunnel_id, other_tunnel);
    assert_eq!(allocation_tunnel_id, allocation_tunnel);
    Ok(())
}

// ---------------------------------------------------------------------------
// INV-5 -- audit metadata holds no secrets and no raw guest IPs
// ---------------------------------------------------------------------------

fn meta(pairs: &[(&str, &str)]) -> std::result::Result<audit::AuditMetadata, String> {
    let entries = pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), serde_json::Value::String((*v).to_owned())));
    audit::build(entries).map_err(|e| e.to_string())
}

#[test]
fn inv5_accepts_allowed_metadata() -> TestResult {
    // Passing cases: the keys this crate writes, with ordinary values.
    let built = meta(&[
        ("reason", "owner_requested"),
        ("state_before", "active"),
        ("state_after", "stopped"),
        ("count", "3"),
    ])?;
    assert_eq!(
        built.as_value().as_object().map(serde_json::Map::len),
        Some(4)
    );

    // Empty metadata is valid, and is the default the column already carries.
    let empty = audit::build(std::iter::empty::<(String, serde_json::Value)>())?;
    assert_eq!(empty, audit::AuditMetadata::default());
    Ok(())
}

#[test]
fn inv5_rejects_a_raw_guest_ip_however_it_arrives() {
    // Failing-input case, and the one that matters most: PRIV-002 forbids raw guest IPs in
    // logs and audit metadata, and `source_ip` is real personal data. The key here is on
    // the allow-list, so only the value check can catch this -- which is exactly why the
    // value check exists and is tested separately from the key check.
    for address in [
        "203.0.113.5",
        "10.0.0.1",
        "2001:db8::1",
        "::ffff:203.0.113.5",
    ] {
        let outcome = meta(&[("reason", address)]);
        assert!(
            outcome.is_err(),
            "audit metadata accepted {address:?}, which is a raw IP address (PRIV-002)"
        );
        // And the rejection must name the rule without echoing the value.
        let message = outcome.err().unwrap_or_default();
        assert!(
            !message.contains(address),
            "the rejection message leaked the address: {message}"
        );
    }
}

#[test]
fn inv5_rejects_keys_that_are_not_allowed() {
    // Failing-input cases: keys outside the allow-list, including the ones that would be
    // obvious smuggling routes.
    for key in [
        "token",
        "refresh_token",
        "password",
        "secret",
        "source_ip",
        "payload",
        "body",
        "email",
        "reason_token",
    ] {
        let outcome = meta(&[(key, "anything")]);
        assert!(
            outcome.is_err(),
            "audit metadata accepted the key {key:?}, which is not on the allow-list"
        );
    }
}

#[test]
fn inv5_rejects_nested_and_oversized_values() {
    // Failing-input cases: a payload does not arrive as a string, it arrives nested.
    let nested = audit::build([(
        "count".to_owned(),
        serde_json::json!({"bytes": 1, "token": "x"}),
    )]);
    assert!(nested.is_err(), "audit metadata accepted a nested object");

    let array = audit::build([("count".to_owned(), serde_json::json!([1, 2, 3]))]);
    assert!(array.is_err(), "audit metadata accepted an array");

    // A long opaque string is what a token looks like.
    let long = "A".repeat(512);
    assert!(
        meta(&[("reason", &long)]).is_err(),
        "audit metadata accepted a 512-character value"
    );
}

#[test]
fn inv5_key_shape_matches_the_project_identifier_rule() {
    // The allow-list is the primary control, so these are belt and braces. Both must hold.
    for good in ["reason", "state_before", "count", "a", "a1_b2"] {
        assert!(
            audit::is_project_identifier(good),
            "{good:?} should be accepted"
        );
    }
    for bad in [
        "Reason",
        "1reason",
        "reason-text",
        "reason.text",
        "",
        "r\u{e9}ason",
    ] {
        assert!(
            !audit::is_project_identifier(bad),
            "{bad:?} should be refused"
        );
    }
}

// ---------------------------------------------------------------------------
// Bounded-input behaviour, which AGENTS.md section 4 requires of every entry point
// ---------------------------------------------------------------------------

#[test]
fn source_ip_validation_accepts_hosts_and_rejects_everything_else() {
    // This tests `assert_valid_source_ip` itself rather than re-testing `IpAddr`, because
    // the interesting property is the *divergence* from the column it guards.
    //
    // `guest_admissions.source_ip` is a PostgreSQL `inet`, which accepts CIDR notation:
    // `203.0.113.0/24` is a perfectly valid inet value. This layer refuses it, on
    // purpose. An admission grants relay access to one guest address; admitting a network
    // would widen a single guest's grant into a whole subnet, which is a security-relevant
    // broadening and not something a caller should be able to do by formatting an address
    // differently.
    //
    // The previous version of this test asserted that `203.0.113.5/32` parsed as an
    // `IpAddr`, which is false -- Rust rejects the prefix -- and that assertion took the
    // `test` job red. It was testing the wrong thing: re-asserting a stdlib's parser
    // instead of this module's contract.
    for accepted in [
        "203.0.113.5", // IPv4, the documentation range
        "10.0.0.1",
        "127.0.0.1",
        "2001:db8::1", // IPv6
        "::1",
    ] {
        assert!(
            sharing::assert_valid_source_ip(accepted).is_ok(),
            "{accepted:?} is a host address and must be accepted"
        );
    }

    for rejected in [
        "", // empty
        "not-an-ip",
        "203.0.113.999", // out of range
        "999.999.999.999",
        "203.0.113.0/24", // a network, not a host
        "203.0.113.5/32",
        "203.0.113.5/99",    // invalid prefix length
        " 203.0.113.5 ",     // surrounding whitespace
        "203.0.113.5:25565", // a socket address, which is not a peer address
    ] {
        assert!(
            sharing::assert_valid_source_ip(rejected).is_err(),
            "{rejected:?} must be refused as a guest source address"
        );
    }
}
