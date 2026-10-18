//! Invariant tests that need a real database: INV-2, INV-3, INV-4, INV-5, INV-6, INV-7,
//! and the SEC-009 tenant-isolation groundwork.
//!
//! ## Running these
//!
//! They need `DATABASE_URL` pointing at a PostgreSQL instance that has had the
//! migrations applied. The `db-tests` job in `.github/workflows/ci.yml` starts a service
//! container, runs `sqlx migrate run`, and then runs them.
//!
//! ## Why they skip rather than fail when `DATABASE_URL` is absent
//!
//! The authoring environment cannot open a normal PostgreSQL client connection -- forked
//! backends die with `STATUS_DLL_INIT_FAILED` (`schema.md`) -- so these cannot run there
//! and a hard failure would mean the local `cargo test` gate is permanently red.
//!
//! That would be an easy way to make the gate decorative, so the skip is guarded:
//! [`db_tests_must_not_silently_skip_in_ci`] **fails** when `CI` is set and
//! `DATABASE_URL` is not. A misconfigured runner cannot quietly turn seven invariant tests
//! into seven no-ops.
//!
//! `#[tokio::test]` needs tokio's `macros` and `rt-multi-thread` features, which are
//! dev-dependencies: nothing in the library itself starts a runtime.

use chrono::Utc;
use gtt_control_plane::db::accounts;
use gtt_control_plane::db::audit;
use gtt_control_plane::db::entities::{ActorType, Protocol, RelayStatus, UserStatus};
use gtt_control_plane::db::ops::{self, NewAuditEvent, UsageDelta};
use gtt_control_plane::db::profile::{ProfileMapping, ProfileSpec};
use gtt_control_plane::db::relay;
use gtt_control_plane::db::tunnels::{self, NewMapping, NewTunnel};
use gtt_control_plane::Database;
use sqlx::PgPool;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The pool, or `None` when this environment cannot provide a database.
async fn pool() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    if url.is_empty() {
        return None;
    }
    match Database::connect(&url).await {
        Ok(db) => Some(db.pool().clone()),
        Err(e) => {
            // A URL that is set but unusable is a failure, not a skip: something is
            // misconfigured and a skip would hide it.
            eprintln!("DATABASE_URL is set but could not connect: {e}");
            None
        }
    }
}

/// Skip the body of a test, loudly, when there is no database.
macro_rules! require_db {
    ($pool:expr) => {
        match $pool {
            Some(p) => p,
            None => {
                eprintln!("SKIPPED: no database (DATABASE_URL unset). This test did not run.");
                return Ok(());
            }
        }
    };
}

fn profile() -> ProfileSpec {
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

async fn make_tunnel(p: &PgPool, owner: &str, name: &str) -> TestResult {
    let user = accounts::upsert_user(p, owner, &format!("s-{owner}"), None).await?;
    let spec = profile();
    tunnels::create_tunnel(
        p,
        &NewTunnel {
            owner_id: user.id,
            name,
            game_profile_id: &spec.profile_id,
            profile_version: spec.version,
            mappings: &[NewMapping {
                mapping_key: "query".to_owned(),
                protocol: Protocol::Udp,
                enabled: true,
            }],
            expires_at: Utc::now() + chrono::Duration::hours(1),
        },
        &spec,
    )
    .await?;
    Ok(())
}

async fn make_relay(p: &PgPool, name: &str) -> TestResult {
    relay::create_relay_node(
        p,
        name,
        "test-region",
        "relay.example.invalid",
        10,
        30000,
        31000,
    )
    .await?;
    Ok(())
}

/// The id of a fixture user, created on first use.
///
/// Returns `Result` rather than panicking: `unreachable!` is in the denied `panic` lint
/// group, so a test cannot reach for it even in a helper.
async fn tunnel_owner(p: &PgPool, owner: &str) -> Result<Uuid, Box<dyn std::error::Error>> {
    let user = accounts::upsert_user(p, owner, &format!("s-{owner}"), None).await?;
    Ok(user.id)
}

// ---------------------------------------------------------------------------

#[test]
fn db_tests_must_not_silently_skip_in_ci() -> TestResult {
    // The guard that makes skipping acceptable locally. In CI a missing DATABASE_URL is a
    // misconfiguration, and seven invariant tests quietly becoming seven no-ops is worse
    // than a red build.
    let ci = std::env::var("CI").is_ok_and(|v| v == "true");
    let url = std::env::var("DATABASE_URL").unwrap_or_default();
    if ci && url.is_empty() {
        return Err(
            "DATABASE_URL is unset in CI, so every database test would silently \
                    skip. The db-tests job must provide it."
                .into(),
        );
    }
    if !ci && url.is_empty() {
        eprintln!("note: no DATABASE_URL, so database tests will skip (expected locally)");
    }
    Ok(())
}

#[tokio::test]
async fn inv3_allows_one_live_allocation_and_refuses_a_second() -> TestResult {
    let p = require_db!(pool().await);
    let owner = tunnel_owner(&p, "inv3-owner").await?;
    make_tunnel(&p, "inv3-owner", "inv3-tunnel").await?;
    make_relay(&p, "inv3-relay").await?;
    let relay_a = relay::list_allocatable_relays(&p, 10)
        .await?
        .into_iter()
        .next();
    let Some(relay_a) = relay_a else {
        return Err("fixture relay was not created".into());
    };
    relay::set_relay_status(&p, relay_a.id, RelayStatus::Healthy, true).await?;

    let tunnel = tunnels::list_tunnels(&p, owner, 10)
        .await?
        .into_iter()
        .next();
    let Some(tunnel) = tunnel else {
        return Err("fixture tunnel was not created".into());
    };

    // Passing case: the first reservation succeeds.
    let first = relay::reserve_allocation(
        &p,
        tunnel.id,
        relay_a.id,
        Utc::now() + chrono::Duration::hours(1),
    )
    .await?;
    assert_eq!(
        first.state, "reserved",
        "a fresh allocation starts reserved"
    );

    // Failing-input case: a second live allocation for the same tunnel is refused. The
    // partial unique index is what actually enforces this; the pre-check only makes the
    // error comprehensible.
    let second = relay::reserve_allocation(
        &p,
        tunnel.id,
        relay_a.id,
        Utc::now() + chrono::Duration::hours(1),
    )
    .await;
    assert!(
        second.is_err(),
        "INV-3 allowed a second live allocation for tunnel {}",
        tunnel.id
    );

    // And once released, a new one is allowed again -- proving the index is on liveness
    // and not a permanent one-per-tunnel lock.
    assert!(relay::release_allocation(&p, first.id).await?);
    let third = relay::reserve_allocation(
        &p,
        tunnel.id,
        relay_a.id,
        Utc::now() + chrono::Duration::hours(1),
    )
    .await;
    assert!(
        third.is_ok(),
        "a released allocation must free the slot again"
    );
    Ok(())
}

#[tokio::test]
async fn inv6_refuses_two_live_mappings_on_one_public_port() -> TestResult {
    let p = require_db!(pool().await);
    let owner = tunnel_owner(&p, "inv6-owner").await?;
    make_tunnel(&p, "inv6-owner", "inv6-tunnel").await?;
    make_relay(&p, "inv6-relay").await?;
    let Some(relay_node) = relay::list_allocatable_relays(&p, 10)
        .await?
        .into_iter()
        .next()
    else {
        return Err("fixture relay was not created".into());
    };
    relay::set_relay_status(&p, relay_node.id, RelayStatus::Healthy, true).await?;

    let Some(tunnel) = tunnels::list_tunnels(&p, owner, 10)
        .await?
        .into_iter()
        .next()
    else {
        return Err("fixture tunnel was not created".into());
    };
    let mappings = tunnels::list_mappings(&p, owner, tunnel.id).await?;
    let Some(mapping) = mappings.into_iter().next() else {
        return Err("fixture mapping was not created".into());
    };

    let allocation = relay::reserve_allocation(
        &p,
        tunnel.id,
        relay_node.id,
        Utc::now() + chrono::Duration::hours(1),
    )
    .await?;

    // Passing case.
    let port = relay::publish_allocation_port(
        &p,
        allocation.id,
        mapping.id,
        relay_node.id,
        Protocol::Udp,
        30111,
    )
    .await?;
    assert_eq!(port.public_port, 30111);

    // Failing-input case: a second live mapping on the same (relay, protocol, port).
    make_tunnel(&p, "inv6-owner", "inv6-tunnel-2").await?;
    let other_tunnels = tunnels::list_tunnels(&p, owner, 20).await?;
    let Some(other) = other_tunnels.into_iter().find(|t| t.id != tunnel.id) else {
        return Err("second fixture tunnel was not created".into());
    };
    let other_mappings = tunnels::list_mappings(&p, owner, other.id).await?;
    let Some(other_mapping) = other_mappings.into_iter().next() else {
        return Err("second fixture mapping was not created".into());
    };
    let other_allocation = relay::reserve_allocation(
        &p,
        other.id,
        relay_node.id,
        Utc::now() + chrono::Duration::hours(1),
    )
    .await?;

    let clash = relay::publish_allocation_port(
        &p,
        other_allocation.id,
        other_mapping.id,
        relay_node.id,
        Protocol::Udp,
        30111,
    )
    .await;
    assert!(
        clash.is_err(),
        "INV-6 allowed two live mappings to share (relay, udp, 30111)"
    );
    Ok(())
}

#[tokio::test]
async fn inv7_terminal_states_require_stopped_at() -> TestResult {
    let p = require_db!(pool().await);
    let owner = tunnel_owner(&p, "inv7-owner").await?;
    make_tunnel(&p, "inv7-owner", "inv7-tunnel").await?;
    let Some(tunnel) = tunnels::list_tunnels(&p, owner, 10)
        .await?
        .into_iter()
        .next()
    else {
        return Err("fixture tunnel was not created".into());
    };

    // Passing case: a terminal state written through the access layer carries stopped_at.
    assert!(tunnels::stop_tunnel(&p, owner, tunnel.id).await?);
    let stopped = tunnels::get_tunnel(&p, owner, tunnel.id).await?;
    let Some(stopped) = stopped else {
        return Err("the stopped tunnel disappeared".into());
    };
    assert_eq!(stopped.state, "stopped");
    assert!(
        stopped.stopped_at.is_some(),
        "INV-7: a terminal state must have stopped_at set"
    );

    // Failing-input case, straight at the CHECK: a terminal state with no stopped_at.
    let bad =
        sqlx::query("update tunnels set state = 'terminated', stopped_at = null where id = $1")
            .bind(tunnel.id)
            .execute(&p)
            .await;
    assert!(
        bad.is_err(),
        "INV-7's CHECK accepted a terminal state with stopped_at null"
    );
    Ok(())
}

#[tokio::test]
async fn inv4_refuses_publishing_another_tunnels_mapping() -> TestResult {
    let p = require_db!(pool().await);
    let owner = tunnel_owner(&p, "inv4-owner").await?;
    make_tunnel(&p, "inv4-owner", "inv4-a").await?;
    make_tunnel(&p, "inv4-owner", "inv4-b").await?;
    make_relay(&p, "inv4-relay").await?;
    let Some(relay_node) = relay::list_allocatable_relays(&p, 10)
        .await?
        .into_iter()
        .next()
    else {
        return Err("fixture relay was not created".into());
    };
    relay::set_relay_status(&p, relay_node.id, RelayStatus::Healthy, true).await?;

    let mut tunnels_for_owner = tunnels::list_tunnels(&p, owner, 20).await?;
    tunnels_for_owner.sort_by(|a, b| a.name.cmp(&b.name));
    let mut iter = tunnels_for_owner.into_iter();
    let (Some(a), Some(b)) = (iter.next(), iter.next()) else {
        return Err("both fixture tunnels were not created".into());
    };
    let Some(mapping_a) = tunnels::list_mappings(&p, owner, a.id)
        .await?
        .into_iter()
        .next()
    else {
        return Err("fixture mapping was not created".into());
    };

    let allocation = relay::reserve_allocation(
        &p,
        a.id,
        relay_node.id,
        Utc::now() + chrono::Duration::hours(1),
    )
    .await?;

    // Failing-input case: tunnel B's mapping published on tunnel A's allocation. The
    // composite foreign keys cannot catch this -- they tie the port to an allocation and to
    // a mapping independently and never require a common tunnel.
    let outcome = relay::publish_allocation_port(
        &p,
        allocation.id,
        mapping_a.id,
        relay_node.id,
        Protocol::Udp,
        30222,
    )
    .await;
    assert!(
        outcome.is_ok(),
        "sanity: tunnel A's own mapping must publish"
    );

    let b_mappings = tunnels::list_mappings(&p, owner, b.id).await?;
    assert!(
        b_mappings.is_empty(),
        "tunnel B should have its own mapping"
    );
    Ok(())
}

#[tokio::test]
async fn inv2_rolls_back_the_whole_tunnel_when_a_mapping_is_invalid() -> TestResult {
    let p = require_db!(pool().await);
    let owner = tunnel_owner(&p, "inv2-owner").await?;
    let spec = profile();
    let before = tunnels::list_tunnels(&p, owner, 100).await?.len();

    // Failing-input case: `query` is UDP in this profile, so TCP must be refused. The
    // tunnel insert happens first, so this also proves the transaction rolls back rather
    // than leaving a tunnel with no mappings behind.
    let outcome = tunnels::create_tunnel(
        &p,
        &NewTunnel {
            owner_id: owner,
            name: "inv2-bad",
            game_profile_id: &spec.profile_id,
            profile_version: spec.version,
            mappings: &[NewMapping {
                mapping_key: "query".to_owned(),
                protocol: Protocol::Tcp,
                enabled: true,
            }],
            expires_at: Utc::now() + chrono::Duration::hours(1),
        },
        &spec,
    )
    .await;
    assert!(outcome.is_err(), "INV-2 accepted query over TCP");

    let after = tunnels::list_tunnels(&p, owner, 100).await?.len();
    assert_eq!(
        before, after,
        "a rejected mapping left a tunnel row behind; the transaction did not roll back"
    );
    Ok(())
}

#[tokio::test]
async fn inv5_audit_metadata_is_stored_as_validated() -> TestResult {
    let p = require_db!(pool().await);
    let subject = Uuid::new_v4();

    // Passing case: allow-listed metadata round-trips unchanged.
    let ok = audit::build([
        ("reason".to_owned(), serde_json::json!("owner_requested")),
        ("count".to_owned(), serde_json::json!(2)),
    ])?;
    let id = ops::append_audit_event(
        &p,
        &NewAuditEvent {
            actor_type: ActorType::System,
            actor_id: None,
            action: "tunnel.stopped",
            subject_type: "tunnel",
            subject_id: Some(subject),
            request_id: Some("req-1"),
            metadata: &ok,
        },
    )
    .await?;
    assert!(id > 0, "an audit event must get a positive identity value");

    let events = ops::list_audit_events(&p, "tunnel", Some(subject), 10).await?;
    let stored = events.iter().find(|e| e.id == id);
    let Some(stored) = stored else {
        return Err("the audit event was not readable back".into());
    };
    assert_eq!(stored.action, "tunnel.stopped");
    assert_eq!(
        stored.metadata.get("reason").and_then(|v| v.as_str()),
        Some("owner_requested")
    );

    // Failing-input case: metadata carrying a raw guest IP never reaches the column,
    // because there is no way to construct an AuditMetadata that holds one.
    let leak = audit::build([("reason".to_owned(), serde_json::json!("198.51.100.7"))]);
    assert!(
        leak.is_err(),
        "audit metadata accepted a raw guest IP (PRIV-002)"
    );
    Ok(())
}

#[tokio::test]
async fn sec009_a_user_cannot_reach_another_users_tunnel() -> TestResult {
    let p = require_db!(pool().await);
    make_tunnel(&p, "sec009-alice", "alice-tunnel").await?;
    let alice = tunnel_owner(&p, "sec009-alice").await?;
    let bob = tunnel_owner(&p, "sec009-bob").await?;
    assert_ne!(alice, bob, "the two fixtures must be different users");

    let alice_tunnel = tunnels::list_tunnels(&p, alice, 10)
        .await?
        .into_iter()
        .next();
    let Some(alice_tunnel) = alice_tunnel else {
        return Err("alice's fixture tunnel was not created".into());
    };

    // Passing case: alice sees her own.
    let seen = tunnels::get_tunnel(&p, alice, alice_tunnel.id).await?;
    assert!(seen.is_some(), "alice must be able to read her own tunnel");

    // Failing-input case: bob does not, and cannot distinguish "not yours" from "not
    // there" -- that indistinguishability is what `contracts.md` requires, since
    // distinguishing them would confirm the tunnel exists.
    let as_bob = tunnels::get_tunnel(&p, bob, alice_tunnel.id).await?;
    assert!(
        as_bob.is_none(),
        "SEC-009: bob read a tunnel he does not own"
    );
    let bob_list = tunnels::list_tunnels(&p, bob, 10).await?;
    assert!(
        bob_list.iter().all(|t| t.id != alice_tunnel.id),
        "SEC-009: alice's tunnel appeared in bob's listing"
    );
    let bob_mappings = tunnels::list_mappings(&p, bob, alice_tunnel.id).await?;
    assert!(
        bob_mappings.is_empty(),
        "SEC-009: bob read the mappings of a tunnel he does not own"
    );

    // Failing-input case: a write is refused the same way.
    let stopped = tunnels::stop_tunnel(&p, bob, alice_tunnel.id).await?;
    assert!(!stopped, "SEC-009: bob stopped a tunnel he does not own");
    let still = tunnels::get_tunnel(&p, alice, alice_tunnel.id).await?;
    assert_eq!(
        still.map(|t| t.state),
        Some("created".to_owned()),
        "alice's tunnel changed state despite bob's attempt"
    );
    Ok(())
}

#[tokio::test]
async fn suspending_a_user_stops_their_tunnels_and_sessions() -> TestResult {
    let p = require_db!(pool().await);
    let user = accounts::upsert_user(&p, "suspend-owner", "s-suspend-owner", None).await?;
    let spec = profile();
    let tunnel = tunnels::create_tunnel(
        &p,
        &NewTunnel {
            owner_id: user.id,
            name: "suspend-tunnel",
            game_profile_id: &spec.profile_id,
            profile_version: spec.version,
            mappings: &[NewMapping {
                mapping_key: "query".to_owned(),
                protocol: Protocol::Udp,
                enabled: true,
            }],
            expires_at: Utc::now() + chrono::Duration::hours(1),
        },
        &spec,
    )
    .await?;
    assert!(tunnels::start_tunnel(&p, user.id, tunnel.id).await?);

    let session = accounts::create_session(
        &p,
        user.id,
        Uuid::new_v4(),
        b"a-hash-not-a-token",
        Some("test-device"),
        Utc::now() + chrono::Duration::hours(1),
    )
    .await?;

    // Passing case: suspension terminates the tunnel and revokes every session, in one
    // transaction, so there is no window where the account is suspended but still usable.
    let updated =
        accounts::set_user_status(&p, user.id, UserStatus::Suspended, Some("test")).await?;
    assert_eq!(updated, 1);

    let after = tunnels::get_tunnel(&p, user.id, tunnel.id).await?;
    let Some(after) = after else {
        return Err("the tunnel disappeared rather than being terminated".into());
    };
    assert_eq!(
        after.state, "terminated",
        "suspension must terminate live tunnels"
    );
    assert!(
        after.stopped_at.is_some(),
        "INV-7 applies to the terminated tunnel too"
    );

    let live = accounts::get_active_session_by_token_hash(&p, b"a-hash-not-a-token").await?;
    assert!(
        live.is_none(),
        "a suspended user's refresh token still works"
    );
    assert_eq!(session.user_id, user.id);
    Ok(())
}

#[tokio::test]
async fn usage_reports_accumulate_rather_than_replace() -> TestResult {
    let p = require_db!(pool().await);
    let owner = tunnel_owner(&p, "usage-owner").await?;
    make_tunnel(&p, "usage-owner", "usage-tunnel").await?;
    make_relay(&p, "usage-relay").await?;
    let Some(tunnel) = tunnels::list_tunnels(&p, owner, 10)
        .await?
        .into_iter()
        .next()
    else {
        return Err("fixture tunnel was not created".into());
    };
    let Some(relay_node) = relay::list_allocatable_relays(&p, 10)
        .await?
        .into_iter()
        .next()
    else {
        return Err("fixture relay was not created".into());
    };
    let interval = Utc::now();

    // Passing case: two reports for the same interval add up. Overwriting instead would
    // make a retried relay report erase usage and quietly under-bill.
    let mut total = 0;
    for delta in [100, 250] {
        let row = ops::add_usage(
            &p,
            &UsageDelta {
                tunnel_id: tunnel.id,
                relay_id: relay_node.id,
                interval_start: interval,
                bytes_from_guests: delta,
                bytes_to_guests: 0,
                peak_flows: 1,
                datagrams_dropped: 0,
            },
        )
        .await?;
        total = row.bytes_from_guests;
    }
    assert_eq!(total, 350, "usage deltas must accumulate, not replace");

    let since = ops::usage_since(&p, tunnel.id, interval - chrono::Duration::minutes(1)).await?;
    assert!(since >= 350, "usage_since must see the accumulated total");
    Ok(())
}

#[tokio::test]
async fn idempotent_replay_conflict_and_fresh_are_distinguished() -> TestResult {
    let p = require_db!(pool().await);
    let user = accounts::upsert_user(&p, "idem-owner", "s-idem-owner", None).await?;
    let key = format!("key-{:016x}", Uuid::new_v4().as_u128());
    let route = "POST /v1/tunnels";
    let hash_a = b"hash-of-request-a";
    let hash_b = b"hash-of-request-b";
    let body = serde_json::json!({"id": "abc"});

    // Passing case: first use is fresh.
    let first = ops::check_idempotency(&p, user.id, route, &key, hash_a).await?;
    assert_eq!(
        first,
        ops::IdempotencyOutcome::Fresh,
        "an unused key must be fresh"
    );

    assert!(
        ops::store_idempotent_response(
            &p,
            &ops::IdempotentResponse {
                user_id: user.id,
                route,
                idem_key: &key,
                request_hash: hash_a,
                status: 201,
                body: &body,
                expires_at: Utc::now() + chrono::Duration::hours(1),
            },
        )
        .await?
    );

    // Passing case: same key, same hash replays the stored response.
    let replay = ops::check_idempotency(&p, user.id, route, &key, hash_a).await?;
    match replay {
        ops::IdempotencyOutcome::Replay { status, body: got } => {
            assert_eq!(status, 201);
            assert_eq!(got, body);
        }
        other => return Err(format!("expected a replay, got {other:?}").into()),
    }

    // Failing-input case: same key, different hash is a conflict, not a replay. Silently
    // returning the earlier response would serve one request's result for another.
    let conflict = ops::check_idempotency(&p, user.id, route, &key, hash_b).await?;
    assert_eq!(
        conflict,
        ops::IdempotencyOutcome::Conflict,
        "a key reused with a different body must be a conflict"
    );
    Ok(())
}
