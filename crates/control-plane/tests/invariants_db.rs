//! Invariant tests that need a real database: INV-2, INV-3, INV-4, INV-5, INV-6, INV-7,
//! and the SEC-009 tenant-isolation groundwork.
//!
//! ## Running these
//!
//! They need `DATABASE_URL` pointing at a PostgreSQL instance that has had the migrations
//! applied. The `db-tests` job in `.github/workflows/ci.yml` starts a service container,
//! runs `sqlx migrate run`, and then runs them.
//!
//! ## Why a promised database can never become a skip
//!
//! These cannot run in the authoring environment: it cannot open a normal PostgreSQL client
//! connection (forked backends die with `STATUS_DLL_INIT_FAILED`, see `schema.md`), and it
//! cannot link a test binary either. A hard failure there would mean the local `cargo test`
//! gate is permanently red, so they skip when no database is available.
//!
//! That arrangement has already produced **one green CI job in which all eleven tests
//! skipped and reported "11 passed"**, so the skip path is fenced off three ways:
//!
//! 1. `REQUIRE_DB_TESTS=true` is set by the `db-tests` job. It is deliberately **not** `CI`:
//!    the plain `test` job also runs in CI and has no database service by design.
//! 2. A **connection failure** is fatal whenever a database was promised. An earlier
//!    version turned it into a skip, which is how the green-but-empty job happened.
//! 3. `require_db!` asserts rather than skipping, so a test cannot report success while
//!    having done nothing. A separate CI step reads the output as an independent second net.
//!
//! ## Parallelism
//!
//! Every test in this file shares one database and they run in parallel, so **no test may
//! look up a fixture by listing**. `make_relay` returns the node it created for exactly this
//! reason: a relay defaults to `provisioning`, only `healthy` relays are allocatable, and
//! "the first healthy relay" would be whichever test got there first.
//!
//! `#[tokio::test]` needs tokio's `macros` and `rt-multi-thread` features, which are
//! dev-dependencies: nothing in the library itself starts a runtime.

use chrono::Utc;
use gtt_control_plane::db::accounts;
use gtt_control_plane::db::audit;
use gtt_control_plane::db::entities::{self, ActorType, Protocol, RelayStatus, UserStatus};
use gtt_control_plane::db::ops::{self, NewAuditEvent, UsageDelta};
use gtt_control_plane::db::profile::{ProfileMapping, ProfileSpec};
use gtt_control_plane::db::relay;
use gtt_control_plane::db::sharing;
use gtt_control_plane::db::tunnels::{self, NewMapping, NewTunnel};
use gtt_control_plane::policy;
use gtt_control_plane::Database;
use sqlx::PgPool;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// For helpers that return an id. Separate from [`TestResult`] because that one is
/// `Result<(), _>` and reusing it here is a type error rather than a silent bug.
type IdResult = Result<Uuid, Box<dyn std::error::Error>>;

/// Whether this job promised a database. Set only by the `db-tests` CI job.
fn database_required() -> bool {
    std::env::var("REQUIRE_DB_TESTS").is_ok_and(|v| v == "true")
}

/// The pool, or `None` only when no database was promised.
///
/// **A connection failure is never a skip.** An earlier version returned `None` here when
/// `Database::connect` failed, printing a note to stderr and carrying on. Combined with the
/// `SET statement_timeout = $1` bind-parameter bug — which made *every* connect fail —
/// that produced a green CI job in which all eleven tests skipped and reported "11 passed".
/// A test that cannot reach its database has failed; it has not skipped.
async fn pool() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").unwrap_or_default();
    if url.is_empty() {
        assert!(
            !database_required(),
            "REQUIRE_DB_TESTS=true but DATABASE_URL is unset"
        );
        return None;
    }
    match Database::connect(&url).await {
        Ok(db) => Some(db.pool().clone()),
        Err(e) => {
            eprintln!("FATAL: DATABASE_URL is set but the connection failed: {e}");
            assert!(
                !database_required(),
                "REQUIRE_DB_TESTS=true but the database could not be reached: {e}"
            );
            None
        }
    }
}

/// Require a pool, failing the test when one was promised.
///
/// The skip branch exists only for environments that never claimed a database, and it says
/// so out loud. This is the structural guard: with it, a test **cannot report success while
/// having done nothing** when the job said a database was available.
macro_rules! require_db {
    ($pool:expr) => {
        match $pool {
            Some(p) => p,
            None => {
                assert!(
                    !database_required(),
                    "REQUIRE_DB_TESTS=true, so this test must run rather than skip"
                );
                eprintln!(
                    "SKIPPED: no database was promised (REQUIRE_DB_TESTS unset). \
                     This test did not run."
                );
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

/// A user's id, created on first use.
///
/// Returns `Result` rather than panicking: `unreachable!` is in the denied `panic` lint
/// group, so a test cannot reach for it even in a helper.
async fn user_id(p: &PgPool, issuer: &str) -> IdResult {
    let user = accounts::upsert_user(p, issuer, &format!("s-{issuer}"), None).await?;
    Ok(user.id)
}

/// A tunnel with one `query` mapping, and its id.
async fn make_tunnel(p: &PgPool, owner: &str, name: &str) -> IdResult {
    let owner_id = user_id(p, owner).await?;
    let spec = profile();
    let tunnel = tunnels::create_tunnel(
        p,
        &NewTunnel {
            owner_id,
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
    Ok(tunnel.id)
}

/// Create a relay node and promote it to `healthy`, returning it.
///
/// Returns the node rather than making the caller go and find it. `list_allocatable_relays`
/// filters on `status = 'healthy'` and a new node defaults to `provisioning`, so a fixture
/// that looked itself up before promoting itself found nothing — which is what the first CI
/// run of these tests did, four times over, with an error message that blamed the wrong
/// thing ("fixture relay was not created", when it had been created and merely was not yet
/// allocatable).
/// A tunnel carrying both a UDP and a TCP mapping.
///
/// Needed by INV-6's "same numeric port, different protocol" case.
/// `allocation_ports` has a composite foreign key `(mapping_id, protocol)` to
/// `tunnel_mappings`, so a mapping's protocol is **not** a free choice at the call site:
/// asking to publish a UDP-only mapping as TCP violates the FK, which is exactly what the
/// first version of that assertion did. The database was right; the test was wrong.
async fn make_tunnel_with_both_protocols(p: &PgPool, owner: &str, name: &str) -> IdResult {
    let owner_id = user_id(p, owner).await?;
    let spec = profile();
    let tunnel = tunnels::create_tunnel(
        p,
        &NewTunnel {
            owner_id,
            name,
            game_profile_id: &spec.profile_id,
            profile_version: spec.version,
            mappings: &[
                NewMapping {
                    mapping_key: "query".to_owned(),
                    protocol: Protocol::Udp,
                    enabled: true,
                },
                NewMapping {
                    mapping_key: "rccon".to_owned(),
                    protocol: Protocol::Tcp,
                    enabled: false,
                },
            ],
            expires_at: Utc::now() + chrono::Duration::hours(1),
        },
        &spec,
    )
    .await?;
    Ok(tunnel.id)
}

async fn make_relay(
    p: &PgPool,
    name: &str,
) -> Result<entities::RelayNode, Box<dyn std::error::Error>> {
    let node = relay::create_relay_node(
        p,
        name,
        "test-region",
        "relay.example.invalid",
        10,
        30000,
        31000,
    )
    .await?;
    relay::set_relay_status(p, node.id, RelayStatus::Healthy, true).await?;
    Ok(node)
}

// ---------------------------------------------------------------------------

#[test]
fn db_tests_must_not_silently_skip_when_a_database_is_required() -> TestResult {
    // The guard that makes skipping acceptable. When the job has declared that a database is
    // required, a missing DATABASE_URL is a misconfiguration, and seven invariant tests
    // quietly becoming seven no-ops is worse than a red build.
    //
    // Keyed on REQUIRE_DB_TESTS, **not** on CI. The first version used CI and fired in the
    // plain `test` job, which has no database service by design, taking that job red on its
    // first run. `CI` says where the code is running; it does not say whether this job is
    // supposed to have a database.
    let required = database_required();
    let url = std::env::var("DATABASE_URL").unwrap_or_default();

    if required && url.is_empty() {
        return Err(
            "REQUIRE_DB_TESTS=true but DATABASE_URL is unset, so every database test \
             would silently skip. The db-tests job must provide both."
                .into(),
        );
    }
    if required {
        eprintln!("REQUIRE_DB_TESTS=true: the database tests will run");
    } else {
        eprintln!(
            "REQUIRE_DB_TESTS is not set: database tests skip unless DATABASE_URL is \
             present. Expected in the plain test job and on a developer machine."
        );
    }
    Ok(())
}

#[tokio::test]
async fn inv3_allows_one_live_allocation_and_refuses_a_second() -> TestResult {
    let p = require_db!(pool().await);
    let tunnel_id = make_tunnel(&p, "inv3-owner", "inv3-tunnel").await?;
    let node = make_relay(&p, "inv3-relay").await?;
    let expires = Utc::now() + chrono::Duration::hours(1);

    // Passing case: the first reservation succeeds.
    let first = relay::reserve_allocation(&p, tunnel_id, node.id, expires).await?;
    assert_eq!(
        first.state, "reserved",
        "a fresh allocation starts reserved"
    );

    // Failing-input case: a second live allocation for the same tunnel is refused. The
    // partial unique index is the real enforcement; the pre-check only makes the error
    // comprehensible.
    let second = relay::reserve_allocation(&p, tunnel_id, node.id, expires).await;
    assert!(
        second.is_err(),
        "INV-3 allowed a second live allocation for tunnel {tunnel_id}"
    );

    // And once released, a new one is allowed again — proving the index is on liveness and
    // not a permanent one-per-tunnel lock.
    assert!(relay::release_allocation(&p, first.id).await?);
    let third = relay::reserve_allocation(&p, tunnel_id, node.id, expires).await;
    assert!(
        third.is_ok(),
        "a released allocation must free the slot again"
    );
    Ok(())
}

#[tokio::test]
async fn inv6_refuses_two_live_mappings_on_one_public_port() -> TestResult {
    let p = require_db!(pool().await);
    let owner = user_id(&p, "inv6-owner").await?;
    let tunnel_a = make_tunnel_with_both_protocols(&p, "inv6-owner", "inv6-a").await?;
    let tunnel_b = make_tunnel_with_both_protocols(&p, "inv6-owner", "inv6-b").await?;
    let node = make_relay(&p, "inv6-relay").await?;
    let expires = Utc::now() + chrono::Duration::hours(1);

    // Selected by protocol, not by position: `position` 0 is the UDP `query`, and picking
    // "the first mapping" is how the previous version ended up asking to publish a UDP
    // mapping as TCP.
    let mappings_a = tunnels::list_mappings(&p, owner, tunnel_a).await?;
    let mapping_a_udp = mappings_a
        .iter()
        .find(|m| m.protocol == Protocol::Udp.as_str())
        .ok_or("tunnel A has no UDP mapping")?;
    let mapping_a_tcp = mappings_a
        .iter()
        .find(|m| m.protocol == Protocol::Tcp.as_str())
        .ok_or("tunnel A has no TCP mapping")?;
    let mapping_b_udp = tunnels::list_mappings(&p, owner, tunnel_b)
        .await?
        .into_iter()
        .find(|m| m.protocol == Protocol::Udp.as_str())
        .ok_or("tunnel B has no UDP mapping")?;

    let alloc_a = relay::reserve_allocation(&p, tunnel_a, node.id, expires).await?;
    let alloc_b = relay::reserve_allocation(&p, tunnel_b, node.id, expires).await?;

    // Passing case: the first live UDP mapping may hold the port.
    let port = relay::publish_allocation_port(
        &p,
        alloc_a.id,
        mapping_a_udp.id,
        node.id,
        Protocol::Udp,
        30111,
    )
    .await?;
    assert_eq!(port.public_port, 30111);

    // Failing-input case: a second live UDP mapping on the same (relay, protocol, port).
    let clash = relay::publish_allocation_port(
        &p,
        alloc_b.id,
        mapping_b_udp.id,
        node.id,
        Protocol::Udp,
        30111,
    )
    .await;
    assert!(
        clash.is_err(),
        "INV-6 allowed two live mappings to share (relay, udp, 30111)"
    );

    // And the same numeric port on a *different* protocol is a different resource, which is
    // what the `protocol` column in the live-port index exists to express. This needs the TCP
    // mapping specifically, because the composite FK will not let a UDP mapping be published
    // as TCP.
    let tcp = relay::publish_allocation_port(
        &p,
        alloc_a.id,
        mapping_a_tcp.id,
        node.id,
        Protocol::Tcp,
        30111,
    )
    .await;
    assert!(
        tcp.is_ok(),
        "the same numeric port on a different protocol must not collide: {tcp:?}"
    );

    // Releasing the UDP port frees it again, proving the index is on unreleased rows.
    assert!(relay::release_allocation_port(&p, alloc_a.id, mapping_a_udp.id).await?);
    let reused = relay::publish_allocation_port(
        &p,
        alloc_b.id,
        mapping_b_udp.id,
        node.id,
        Protocol::Udp,
        30111,
    )
    .await;
    assert!(
        reused.is_ok(),
        "a released port must be reusable: {reused:?}"
    );
    Ok(())
}

#[tokio::test]
async fn inv7_terminal_states_require_stopped_at() -> TestResult {
    let p = require_db!(pool().await);
    let owner = user_id(&p, "inv7-owner").await?;
    let tunnel_id = make_tunnel(&p, "inv7-owner", "inv7-tunnel").await?;

    // Passing case: a terminal state written through the access layer carries stopped_at.
    assert!(tunnels::stop_tunnel(&p, owner, tunnel_id).await?);
    let stopped = tunnels::get_tunnel(&p, owner, tunnel_id)
        .await?
        .ok_or("the stopped tunnel disappeared")?;
    assert_eq!(stopped.state, "stopped");
    assert!(
        stopped.stopped_at.is_some(),
        "INV-7: a terminal state must have stopped_at set"
    );

    // Failing-input case, straight at the CHECK: a terminal state with no stopped_at.
    let bad =
        sqlx::query("update tunnels set state = 'terminated', stopped_at = null where id = $1")
            .bind(tunnel_id)
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
    let owner = user_id(&p, "inv4-owner").await?;
    let tunnel_a = make_tunnel(&p, "inv4-owner", "inv4-a").await?;
    let tunnel_b = make_tunnel(&p, "inv4-owner", "inv4-b").await?;
    let node = make_relay(&p, "inv4-relay").await?;

    let mapping_a = tunnels::list_mappings(&p, owner, tunnel_a)
        .await?
        .into_iter()
        .next()
        .ok_or("tunnel A has no mapping")?;
    let mapping_b = tunnels::list_mappings(&p, owner, tunnel_b)
        .await?
        .into_iter()
        .next()
        .ok_or("tunnel B has no mapping")?;

    let alloc_a = relay::reserve_allocation(
        &p,
        tunnel_a,
        node.id,
        Utc::now() + chrono::Duration::hours(1),
    )
    .await?;

    // Passing case: tunnel A's own mapping publishes on tunnel A's own allocation.
    let own =
        relay::publish_allocation_port(&p, alloc_a.id, mapping_a.id, node.id, Protocol::Udp, 30222)
            .await;
    assert!(own.is_ok(), "sanity: a tunnel's own mapping must publish");

    // Failing-input case: tunnel B's mapping published on tunnel A's allocation. The
    // composite foreign keys cannot catch this — they tie the port to an allocation and to a
    // mapping independently and never require the two to share a tunnel.
    let cross =
        relay::publish_allocation_port(&p, alloc_a.id, mapping_b.id, node.id, Protocol::Udp, 30223)
            .await;
    assert!(
        cross.is_err(),
        "INV-4 allowed tunnel B's mapping to be published on tunnel A's allocation"
    );

    // And nothing was written for the refused attempt.
    let ports = relay::list_allocation_ports(&p, alloc_a.id).await?;
    assert_eq!(
        ports.len(),
        1,
        "the refused INV-4 attempt must not have left a port row behind"
    );
    Ok(())
}

#[tokio::test]
async fn inv2_rolls_back_the_whole_tunnel_when_a_mapping_is_invalid() -> TestResult {
    let p = require_db!(pool().await);
    let owner = user_id(&p, "inv2-owner").await?;
    let spec = profile();
    let before = tunnels::list_tunnels(&p, owner, 100).await?.len();

    // Failing-input case: `query` is UDP in this profile, so TCP must be refused. The tunnel
    // insert happens first, so this also proves the transaction rolls back rather than
    // leaving a tunnel with no mappings behind.
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
    let stored = events
        .iter()
        .find(|e| e.id == id)
        .ok_or("the audit event was not readable back")?;
    assert_eq!(stored.action, "tunnel.stopped");
    assert_eq!(
        stored.metadata.get("reason").and_then(|v| v.as_str()),
        Some("owner_requested")
    );

    // Failing-input case: metadata carrying a raw guest IP never reaches the column, because
    // there is no way to construct an AuditMetadata that holds one.
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
    let alice = make_tunnel(&p, "sec009-alice", "alice-tunnel").await?;
    let bob_id = user_id(&p, "sec009-bob").await?;
    let alice_id = user_id(&p, "sec009-alice").await?;
    assert_ne!(alice_id, bob_id, "the two fixtures must be different users");

    // Passing case: alice sees her own.
    let seen = tunnels::get_tunnel(&p, alice_id, alice).await?;
    assert!(seen.is_some(), "alice must be able to read her own tunnel");

    // Failing-input case: bob does not, and cannot distinguish "not yours" from "not there".
    // That indistinguishability is what `contracts.md` requires, since distinguishing them
    // would confirm the tunnel exists.
    let as_bob = tunnels::get_tunnel(&p, bob_id, alice).await?;
    assert!(
        as_bob.is_none(),
        "SEC-009: bob read a tunnel he does not own"
    );

    let bob_list = tunnels::list_tunnels(&p, bob_id, 10).await?;
    assert!(
        bob_list.iter().all(|t| t.id != alice),
        "SEC-009: alice's tunnel appeared in bob's listing"
    );
    let bob_mappings = tunnels::list_mappings(&p, bob_id, alice).await?;
    assert!(
        bob_mappings.is_empty(),
        "SEC-009: bob read the mappings of a tunnel he does not own"
    );

    // Failing-input case: a write is refused the same way.
    let stopped = tunnels::stop_tunnel(&p, bob_id, alice).await?;
    assert!(!stopped, "SEC-009: bob stopped a tunnel he does not own");
    let still = tunnels::get_tunnel(&p, alice_id, alice).await?;
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

    accounts::create_session(
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

    let after = tunnels::get_tunnel(&p, user.id, tunnel.id)
        .await?
        .ok_or("the tunnel disappeared rather than being terminated")?;
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
    Ok(())
}

#[tokio::test]
async fn usage_reports_accumulate_rather_than_replace() -> TestResult {
    let p = require_db!(pool().await);
    let tunnel_id = make_tunnel(&p, "usage-owner", "usage-tunnel").await?;
    let node = make_relay(&p, "usage-relay").await?;
    let interval = Utc::now();

    // Passing case: two reports for the same interval add up. Overwriting instead would make
    // a retried relay report erase usage and quietly under-bill.
    let mut total = 0;
    for delta in [100, 250] {
        let row = ops::add_usage(
            &p,
            &UsageDelta {
                tunnel_id,
                relay_id: node.id,
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

    let since = ops::usage_since(&p, tunnel_id, interval - chrono::Duration::minutes(1)).await?;
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
    match ops::check_idempotency(&p, user.id, route, &key, hash_a).await? {
        ops::IdempotencyOutcome::Replay { status, body: got } => {
            assert_eq!(status, 201);
            assert_eq!(got, body);
        }
        other => return Err(format!("expected a replay, got {other:?}").into()),
    }

    // Failing-input case: same key, different hash is a conflict, not a replay. Silently
    // returning the earlier response would serve one request's result for another.
    assert_eq!(
        ops::check_idempotency(&p, user.id, route, &key, hash_b).await?,
        ops::IdempotencyOutcome::Conflict,
        "a key reused with a different body must be a conflict"
    );
    let competing_body = serde_json::json!({"id": "must-not-replace"});
    assert!(
        !ops::store_idempotent_response(
            &p,
            &ops::IdempotentResponse {
                user_id: user.id,
                route,
                idem_key: &key,
                request_hash: hash_b,
                status: 202,
                body: &competing_body,
                expires_at: Utc::now() + chrono::Duration::hours(2),
            },
        )
        .await?
    );
    assert_eq!(
        ops::check_idempotency(&p, user.id, route, &key, hash_a).await?,
        ops::IdempotencyOutcome::Replay { status: 201, body }
    );
    Ok(())
}

#[tokio::test]
async fn t042_expired_key_can_be_reused_before_cleanup_without_duplicate_creation() -> TestResult {
    use gtt_control_plane::routes::tunnels::{
        create_tunnel, TunnelRouteConfig, ROUTE_CREATE_TUNNEL,
    };

    let p = require_db!(pool().await);
    let owner = user_id(&p, &format!("t042-expiry-{}", Uuid::new_v4())).await?;
    let actor = policy::Actor::User {
        id: owner,
        status: UserStatus::Active,
    };
    let registry = gtt_profiles::Registry::load_dir(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../profiles"),
    )?;
    let config = TunnelRouteConfig::default();
    let body = r#"{"name":"Original","game_profile_id":"terraria","mappings":[{"mapping_key":"game","protocol":"tcp","enabled":true}]}"#;
    let replacement = r#"{"name":"Replacement","game_profile_id":"terraria","mappings":[{"mapping_key":"game","protocol":"tcp","enabled":true}]}"#;
    let key = "expired-key";
    let original = create_tunnel(
        &p,
        &registry,
        &config,
        actor,
        Some(key),
        body,
        "expiry-original",
    )
    .await
    .map_err(|e| format!("original create: {e:?}"))?;

    // Keep the expired row in place: correctness must not depend on the janitor's schedule.
    // Expire at a statement boundary in an open transaction, so using transaction-start
    // `now()` in the lookup would incorrectly keep this response live.
    let mut tx = p.begin().await?;
    sqlx::query("update idempotency_keys set expires_at = statement_timestamp() where user_id = $1 and route = $2 and idem_key = $3")
        .bind(owner).bind(ROUTE_CREATE_TUNNEL).bind(key).execute(&mut *tx).await?;
    for request in [body, replacement] {
        let parsed =
            gtt_control_plane::routes::tunnels::parse_create_tunnel_request(request, "expiry-hash")
                .map_err(|e| format!("parse: {e:?}"))?;
        let hash = gtt_control_plane::routes::tunnels::hash_create_tunnel_request(&parsed);
        assert_eq!(
            ops::check_idempotency(&mut *tx, owner, ROUTE_CREATE_TUNNEL, key, &hash).await?,
            ops::IdempotencyOutcome::Fresh
        );
    }
    tx.commit().await?;

    let (first, second) = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        tokio::join!(
            create_tunnel(
                &p,
                &registry,
                &config,
                actor,
                Some(key),
                replacement,
                "expiry-first"
            ),
            create_tunnel(
                &p,
                &registry,
                &config,
                actor,
                Some(key),
                replacement,
                "expiry-second"
            )
        )
    })
    .await?;
    let first = first.map_err(|e| format!("first reuse: {e:?}"))?;
    let second = second.map_err(|e| format!("second reuse: {e:?}"))?;
    assert_eq!(first.body, second.body);
    assert_ne!(first.body.id, original.body.id);
    assert_ne!(first.replayed, second.replayed);
    assert_eq!(tunnels::count_live_tunnels(&p, owner).await?, 2);
    let audits: i64 = sqlx::query_scalar(
        "select count(*) from audit_events where actor_id = $1 and action = 'tunnel.created'",
    )
    .bind(owner)
    .fetch_one(&p)
    .await?;
    assert_eq!(audits, 2);

    // The replacement is live: the old body conflicts instead of replacing it again.
    let conflict = create_tunnel(
        &p,
        &registry,
        &config,
        actor,
        Some(key),
        body,
        "expiry-conflict",
    )
    .await
    .err()
    .ok_or("expected a conflict against the replacement")?;
    assert_eq!(
        conflict.code(),
        gtt_protocol::http::ApiErrorCode::IdempotencyMismatch
    );
    let keys: i64 = sqlx::query_scalar("select count(*) from idempotency_keys where user_id = $1")
        .bind(owner)
        .fetch_one(&p)
        .await?;
    assert_eq!(keys, 1);
    Ok(())
}

#[tokio::test]
async fn t042_cleanup_skips_locked_key_and_preserves_renewal() -> TestResult {
    let p = require_db!(pool().await);
    let owner = user_id(&p, &format!("t042-cleanup-{}", Uuid::new_v4())).await?;
    let body = serde_json::json!({"id": "renewed"});
    let route = "POST /v1/tunnels";
    let key = "cleanup-key";
    assert!(
        ops::store_idempotent_response(
            &p,
            &ops::IdempotentResponse {
                user_id: owner,
                route,
                idem_key: key,
                request_hash: b"old",
                status: 201,
                body: &body,
                expires_at: Utc::now() - chrono::Duration::hours(1),
            }
        )
        .await?
    );
    // Hold a row lock like an in-flight reuse. Cleanup must not wait for the lock,
    // and must leave the subsequently renewed response intact.
    let mut tx = p.begin().await?;
    sqlx::query("select user_id from idempotency_keys where user_id = $1 for update")
        .bind(owner)
        .fetch_one(&mut *tx)
        .await?;
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        ops::purge_expired_idempotency_keys(&p, 1000),
    )
    .await??;
    assert!(
        ops::store_idempotent_response(
            &mut *tx,
            &ops::IdempotentResponse {
                user_id: owner,
                route,
                idem_key: key,
                request_hash: b"new",
                status: 201,
                body: &body,
                expires_at: Utc::now() + chrono::Duration::hours(1),
            }
        )
        .await?
    );
    tx.commit().await?;
    ops::purge_expired_idempotency_keys(&p, 1000).await?;
    assert_eq!(
        ops::check_idempotency(&p, owner, route, key, b"new").await?,
        ops::IdempotencyOutcome::Replay { status: 201, body }
    );
    Ok(())
}

#[tokio::test]
async fn a_suspended_user_cannot_start_a_new_tunnel() -> TestResult {
    let p = require_db!(pool().await);
    let user = accounts::upsert_user(&p, "start-owner", "s-start-owner", None).await?;
    accounts::set_user_status(&p, user.id, UserStatus::Suspended, Some("test")).await?;

    // `create_tunnel` does not itself check the user's status: the policy layer decides that
    // when T-041 lands, and SEC-009 is about isolation rather than entitlement. What this
    // test pins is the *data* the policy layer will need — that a suspended user has no live
    // tunnels, so there is nothing for them to start.
    let spec = profile();
    let tunnel = tunnels::create_tunnel(
        &p,
        &NewTunnel {
            owner_id: user.id,
            name: "after-suspension",
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

    let live = tunnels::count_live_tunnels(&p, user.id).await?;
    assert_eq!(
        live, 1,
        "the fixture tunnel is created in the 'created' state, which counts as live"
    );
    // Starting it is what the policy layer will refuse; the data-access layer permits it.
    assert!(
        tunnels::start_tunnel(&p, user.id, tunnel.id).await?,
        "start_tunnel is not the entitlement check; that is T-041's job"
    );
    Ok(())
}

#[tokio::test]
async fn listing_is_bounded_regardless_of_what_the_caller_asks_for() -> TestResult {
    let p = require_db!(pool().await);
    let owner = user_id(&p, "bound-owner").await?;

    // A caller asking for zero, a negative, or an enormous page gets a bounded answer rather
    // than everything or nothing. `AGENTS.md` §4 forbids unbounded allocation on input a
    // caller controls.
    for asked in [-1_i64, 0, 1, 10_000] {
        let got = tunnels::list_tunnels(&p, owner, asked).await?;
        assert!(
            got.len() <= 200,
            "list_tunnels(limit={asked}) returned {} rows; the cap is 200",
            got.len()
        );
    }
    Ok(())
}

#[tokio::test]
async fn expired_tunnels_are_swept_in_bounded_batches() -> TestResult {
    let p = require_db!(pool().await);
    let owner = user_id(&p, "sweep-owner").await?;

    // A raw INSERT, deliberately, because the schema does not permit arranging this any
    // other way. `tunnels_expiry_chk` is `expires_at > created_at`, and `created_at` defaults
    // to the *transaction* start time, which is later than the `Utc::now()` Rust computed a
    // moment earlier -- so a one-second margin is a race, and it lost. Making a tunnel "due"
    // by moving `expires_at` into the past violates the very same CHECK.
    //
    // So both timestamps go in explicitly, two hours and one hour ago: the CHECK holds and the
    // row is already expired. `active` is not terminal, so INV-7 wants `stopped_at` null,
    // which is what is inserted.
    let tunnel_id: Uuid = sqlx::query_scalar(
        "insert into tunnels
             (owner_id, name, game_profile_id, profile_version, state, created_at,
              started_at, expires_at)
         values ($1, 'sweep-tunnel', 'minecraft-java', 1, 'active',
                 now() - interval '2 hours', now() - interval '2 hours',
                 now() - interval '1 hour')
         returning id",
    )
    .bind(owner)
    .fetch_one(&p)
    .await?;

    // Not due yet: a tunnel expiring far in the future must not be swept.
    let far = tunnels::create_tunnel(
        &p,
        &NewTunnel {
            owner_id: owner,
            name: "sweep-future",
            game_profile_id: "minecraft-java",
            profile_version: 1,
            mappings: &[NewMapping {
                mapping_key: "query".to_owned(),
                protocol: Protocol::Udp,
                enabled: true,
            }],
            expires_at: Utc::now() + chrono::Duration::hours(1),
        },
        &profile(),
    )
    .await?;

    // Passing case: the already-due tunnel is swept. The sweep sets `state='expired'` and
    // `stopped_at` in the same statement, so INV-7's CHECK is satisfied by the statement that
    // changes the state rather than needing a second write.
    let swept = tunnels::expire_due_tunnels(&p, 100).await?;
    assert!(
        swept >= 1,
        "the due tunnel should have been swept, swept={swept}"
    );

    let after = tunnels::get_tunnel(&p, owner, tunnel_id)
        .await?
        .ok_or("the swept tunnel disappeared")?;
    assert_eq!(after.state, "expired");
    assert!(
        after.stopped_at.is_some(),
        "INV-7 applies to the sweep too: a terminal state must carry stopped_at"
    );

    let untouched = tunnels::get_tunnel(&p, owner, far.id)
        .await?
        .ok_or("the future tunnel disappeared")?;
    assert_eq!(
        untouched.state, "created",
        "a tunnel that has not expired must not be swept"
    );

    // Idempotent: a second sweep finds nothing more to do.
    let again = tunnels::expire_due_tunnels(&p, 100).await?;
    assert_eq!(again, 0, "a second sweep must find nothing left to expire");
    Ok(())
}

#[tokio::test]
async fn sec009_policy_and_data_layer_cannot_disagree() -> TestResult {
    // The two halves of SEC-009, checked against each other.
    //
    // The data layer scopes every query by owner and reports "not visible" as `None`. The
    // policy turns that into `not_found`. If those two ever disagreed -- the data layer
    // returning a row the policy would refuse, or refusing one it would allow -- there would
    // be a path where a caller sees a resource it should not. So for each operation, the
    // policy's decision and what the data layer actually returns are compared.
    let p = require_db!(pool().await);

    let alice_tunnel = make_tunnel(&p, "sec009-pair-alice", "pair-tunnel").await?;
    let alice = user_id(&p, "sec009-pair-alice").await?;
    let bob = user_id(&p, "sec009-pair-bob").await?;
    assert_ne!(alice, bob, "the two fixtures must be different users");

    let bob_actor = policy::Actor::User {
        id: bob,
        status: UserStatus::Active,
    };

    // (description, operation, did the data layer refuse bob) triples. The policy denies all
    // four with `not_found`; the data layer must independently come back empty. Either half
    // alone would be a claim about one component -- this is where they are held to each other.
    let cases: Vec<(&str, policy::Operation, bool)> = vec![
        ("view", policy::Operation::ViewTunnel, {
            tunnels::get_tunnel(&p, bob, alice_tunnel).await?.is_none()
        }),
        ("list mappings", policy::Operation::ListAdmissions, {
            tunnels::list_mappings(&p, bob, alice_tunnel)
                .await?
                .is_empty()
        }),
        ("start", policy::Operation::StartTunnel, {
            !tunnels::start_tunnel(&p, bob, alice_tunnel).await?
        }),
        ("stop", policy::Operation::StopTunnel, {
            !tunnels::stop_tunnel(&p, bob, alice_tunnel).await?
        }),
    ];

    for (name, op, data_layer_refused) in cases {
        // The policy is asked about a target bob cannot see.
        let decision = policy::decide(bob_actor, op, policy::Target::UserOwned { owner: None });
        assert_eq!(
            decision,
            policy::Decision::Deny(policy::ErrorCode::NotFound),
            "bob must be refused {name} with not_found"
        );
        assert!(
            data_layer_refused,
            "the data layer returned something to bob for {name}, while the policy refused"
        );
    }

    // And the tunnel is untouched.
    let after = tunnels::get_tunnel(&p, alice, alice_tunnel)
        .await?
        .ok_or("alice's tunnel disappeared")?;
    assert_eq!(
        after.state, "created",
        "bob's attempts changed alice's tunnel state"
    );
    Ok(())
}

#[tokio::test]
async fn sec009_an_invitation_cannot_be_revoked_by_another_user() -> TestResult {
    let p = require_db!(pool().await);
    let owner = user_id(&p, "sec009-inv-owner").await?;
    let tunnel_id = make_tunnel(&p, "sec009-inv-owner", "inv-tunnel").await?;
    let stranger = user_id(&p, "sec009-inv-stranger").await?;

    let invitation = sharing::create_invitation(
        &p,
        tunnel_id,
        owner,
        b"a-hash-not-a-secret",
        1,
        Utc::now() + chrono::Duration::hours(1),
    )
    .await?;

    // Failing-input case: a stranger cannot revoke it.
    assert!(
        !sharing::revoke_invitation(&p, stranger, invitation.id).await?,
        "SEC-009: a stranger revoked another user's invitation"
    );

    // Still live for its owner.
    let live = sharing::list_invitations(&p, owner, tunnel_id).await?;
    assert_eq!(live.len(), 1, "the invitation should still exist");
    assert!(
        live.first()
            .map(|i| i.revoked_at)
            .is_none_or(|r| r.is_none()),
        "the stranger's revoke marked it revoked"
    );

    // And the owner can.
    assert!(sharing::revoke_invitation(&p, owner, invitation.id).await?);
    // And not twice.
    assert!(
        !sharing::revoke_invitation(&p, owner, invitation.id).await?,
        "revoking an already-revoked invitation reported success"
    );

    // A stranger cannot even list it.
    let theirs = sharing::list_invitations(&p, stranger, tunnel_id).await?;
    assert!(
        theirs.is_empty(),
        "SEC-009: a stranger listed another user's invitations"
    );
    Ok(())
}

#[tokio::test]
async fn sec009_guest_admissions_are_scoped_to_their_tunnel() -> TestResult {
    let p = require_db!(pool().await);
    let owner = user_id(&p, "sec009-adm-owner").await?;
    let tunnel_a = make_tunnel(&p, "sec009-adm-owner", "adm-a").await?;
    let tunnel_b = make_tunnel(&p, "sec009-adm-owner", "adm-b").await?;

    let invitation_a = sharing::create_invitation(
        &p,
        tunnel_a,
        owner,
        b"hash-a-not-a-secret",
        5,
        Utc::now() + chrono::Duration::hours(1),
    )
    .await?;

    sharing::admit_guest(
        &p,
        invitation_a.id,
        tunnel_a,
        "198.51.100.7",
        Utc::now() + chrono::Duration::hours(1),
    )
    .await?;

    // Each tunnel sees only its own admissions.
    let for_a = sharing::list_live_admissions(&p, tunnel_a, Utc::now()).await?;
    let for_b = sharing::list_live_admissions(&p, tunnel_b, Utc::now()).await?;
    assert_eq!(for_a.len(), 1, "tunnel A should have one admission");
    assert!(
        for_b.is_empty(),
        "tunnel B saw an admission belonging to tunnel A"
    );

    // Re-claiming from the same address refreshes rather than duplicating, which is what the
    // partial unique index on (tunnel_id, source_ip) enforces.
    sharing::admit_guest(
        &p,
        invitation_a.id,
        tunnel_a,
        "198.51.100.7",
        Utc::now() + chrono::Duration::minutes(30),
    )
    .await?;
    let again = sharing::list_live_admissions(&p, tunnel_a, Utc::now()).await?;
    assert_eq!(
        again.len(),
        1,
        "re-claiming from the same address duplicated the admission"
    );

    // Revoking is scoped to the tunnel that holds it.
    let admission_id = again
        .first()
        .map(|a| a.id)
        .ok_or("no admission to revoke")?;
    assert!(!sharing::revoke_admission(&p, tunnel_b, admission_id).await?);
    assert!(sharing::revoke_admission(&p, tunnel_a, admission_id).await?);
    Ok(())
}

// ---------------------------------------------------------------------------
// T-042 -- Routes 6-8 (`POST /v1/tunnels`, `GET /v1/tunnels`, `GET /v1/tunnels/{id}`)
//          with schema validation, INV-2 profile checks, SEC-009 isolation, and
//          idempotency (`contracts.md` §§5.6, 5.7, 8).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn t042_tunnel_routes_enforce_idempotency_replay_mismatch_and_tenant_isolation() -> TestResult
{
    let p = require_db!(pool().await);
    let owner = user_id(&p, "t042-owner").await?;
    let stranger = user_id(&p, "t042-stranger").await?;

    let profiles_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../profiles");
    let registry = gtt_profiles::Registry::load_dir(&profiles_dir)?;
    let config = gtt_control_plane::routes::tunnels::TunnelRouteConfig {
        max_live_tunnels: 2,
        tunnel_ttl_seconds: 3600,
        idempotency_ttl_seconds: 3600,
    };

    let owner_actor = policy::Actor::User {
        id: owner,
        status: UserStatus::Active,
    };
    let stranger_actor = policy::Actor::User {
        id: stranger,
        status: UserStatus::Active,
    };

    let valid_body = r#"{"name":"Friday Terraria","game_profile_id":"terraria","mappings":[{"mapping_key":"game","protocol":"tcp","enabled":true}]}"#;
    let idem_key = format!("idem-{}", Uuid::new_v4());

    // 1. Fresh create returns 201 and a schema-valid Tunnel DTO.
    let created = gtt_control_plane::routes::tunnels::create_tunnel(
        &p,
        &registry,
        &config,
        owner_actor,
        Some(&idem_key),
        valid_body,
        "req_t042_1",
    )
    .await
    .map_err(|e| format!("expected 201 create, got: {e:?}"))?;
    assert_eq!(created.status, 201);
    assert!(!created.replayed);
    created.body.validate()?;

    // 2. Replaying the same Idempotency-Key with the same body returns the exact same tunnel
    //    without creating a second row (`contracts.md` §8).
    let replayed = gtt_control_plane::routes::tunnels::create_tunnel(
        &p,
        &registry,
        &config,
        owner_actor,
        Some(&idem_key),
        valid_body,
        "req_t042_2",
    )
    .await
    .map_err(|e| format!("expected 201 replay, got: {e:?}"))?;
    assert_eq!(replayed.status, 201);
    assert!(replayed.replayed);
    assert_eq!(replayed.body.id, created.body.id);

    // 3. Reusing the same Idempotency-Key with a different request body returns 422
    //    `idempotency_mismatch` (`contracts.md` §8).
    let different_body = r#"{"name":"Saturday Terraria","game_profile_id":"terraria","mappings":[{"mapping_key":"game","protocol":"tcp","enabled":true}]}"#;
    let mismatch = gtt_control_plane::routes::tunnels::create_tunnel(
        &p,
        &registry,
        &config,
        owner_actor,
        Some(&idem_key),
        different_body,
        "req_t042_3",
    )
    .await
    .err()
    .ok_or("expected 422 idempotency_mismatch")?;
    assert_eq!(mismatch.status, 422);
    assert_eq!(
        mismatch.code(),
        gtt_protocol::http::ApiErrorCode::IdempotencyMismatch
    );
    mismatch.response.validate()?;

    // 4. Invalid bodies (unknown profile, wrong protocol for mapping_key) return 400
    //    `validation_failed` per `contracts.md` §5.6.
    for invalid_domain_body in [
        r#"{"name":"Unknown Game","game_profile_id":"no-such-game","mappings":[{"mapping_key":"game","protocol":"tcp","enabled":true}]}"#,
        r#"{"name":"Wrong Proto","game_profile_id":"minecraft-java","mappings":[{"mapping_key":"game","protocol":"udp","enabled":true}]}"#,
        r#"{"name":"Unknown Key","game_profile_id":"minecraft-java","mappings":[{"mapping_key":"voice","protocol":"udp","enabled":true}]}"#,
    ] {
        let err = gtt_control_plane::routes::tunnels::create_tunnel(
            &p,
            &registry,
            &config,
            owner_actor,
            None,
            invalid_domain_body,
            "req_t042_invalid",
        )
        .await
        .err()
        .ok_or_else(|| format!("expected validation_failed for {invalid_domain_body}"))?;
        assert_eq!(err.status, 400);
        assert_eq!(
            err.code(),
            gtt_protocol::http::ApiErrorCode::ValidationFailed
        );
    }

    // 5. GET /v1/tunnels and GET /v1/tunnels/{id} return the tunnel to the owner, and return
    //    404 `not_found` (never 403 `forbidden`) to a stranger (`SEC-009`).
    let listed =
        gtt_control_plane::routes::tunnels::list_tunnels(&p, owner_actor, 50, "req_t042_list")
            .await
            .map_err(|e| format!("list_tunnels failed: {e:?}"))?;
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body.len(), 1);
    assert_eq!(
        listed.body.first().map(|t| t.id.as_str()),
        Some(created.body.id.as_str())
    );

    let fetched = gtt_control_plane::routes::tunnels::get_tunnel(
        &p,
        owner_actor,
        &created.body.id,
        "req_t042_get",
    )
    .await
    .map_err(|e| format!("get_tunnel failed: {e:?}"))?;
    assert_eq!(fetched.status, 200);
    assert_eq!(fetched.body.id, created.body.id);

    let stranger_get = gtt_control_plane::routes::tunnels::get_tunnel(
        &p,
        stranger_actor,
        &created.body.id,
        "req_t042_stranger",
    )
    .await
    .err()
    .ok_or("stranger must not read another user's tunnel")?;
    assert_eq!(stranger_get.status, 404);
    assert_eq!(
        stranger_get.code(),
        gtt_protocol::http::ApiErrorCode::NotFound
    );

    Ok(())
}

/// Concurrent requests must not leave a hidden second tunnel or audit event behind.
#[tokio::test]
async fn t042_concurrent_creates_are_atomic_and_enforce_quota() -> TestResult {
    use gtt_control_plane::routes::tunnels::{create_tunnel, TunnelRouteConfig};
    use gtt_protocol::http::ApiErrorCode;

    let p = require_db!(pool().await);
    let registry = gtt_profiles::Registry::load_dir(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../profiles"),
    )?;
    let body = r#"{"name":"Concurrent","game_profile_id":"terraria","mappings":[{"mapping_key":"game","protocol":"tcp","enabled":true}]}"#;
    let other_body = r#"{"name":"Different","game_profile_id":"terraria","mappings":[{"mapping_key":"game","protocol":"tcp","enabled":true}]}"#;

    // Exercise replay, mismatched bodies, different keys, and keyless quota races.
    for (case, first_key, second_key, second_body, quota) in [
        ("replay", Some("same-key"), Some("same-key"), body, 5),
        (
            "mismatch",
            Some("same-key"),
            Some("same-key"),
            other_body,
            5,
        ),
        ("quota", Some("first-key"), Some("second-key"), body, 1),
        ("keyless-quota", None, None, body, 1),
    ] {
        let owner = user_id(&p, &format!("t042-{case}-{}", Uuid::new_v4())).await?;
        let actor = policy::Actor::User {
            id: owner,
            status: UserStatus::Active,
        };
        let config = TunnelRouteConfig {
            max_live_tunnels: quota,
            ..Default::default()
        };
        let (first, second) = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            tokio::join!(
                create_tunnel(
                    &p,
                    &registry,
                    &config,
                    actor,
                    first_key,
                    body,
                    "concurrent-1"
                ),
                create_tunnel(
                    &p,
                    &registry,
                    &config,
                    actor,
                    second_key,
                    second_body,
                    "concurrent-2"
                )
            )
        })
        .await?;
        if case == "replay" {
            let first = first.map_err(|e| format!("first create: {e:?}"))?;
            let second = second.map_err(|e| format!("second create: {e:?}"))?;
            assert_eq!(first.body, second.body);
            assert_eq!(first.status, 201);
            assert_eq!(second.status, 201);
            assert_ne!(first.replayed, second.replayed);
        } else {
            let expected = if case == "mismatch" {
                ApiErrorCode::IdempotencyMismatch
            } else {
                ApiErrorCode::QuotaExceeded
            };
            let results = [first, second];
            assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
            let error = results
                .iter()
                .find_map(|r| r.as_ref().err())
                .ok_or("expected losing request")?;
            assert_eq!(error.code(), expected);
        }
        assert_eq!(tunnels::count_live_tunnels(&p, owner).await?, 1, "{case}");
        let audits: i64 = sqlx::query_scalar(
            "select count(*) from audit_events where actor_id = $1 and action = 'tunnel.created'",
        )
        .bind(owner)
        .fetch_one(&p)
        .await?;
        assert_eq!(audits, 1, "{case}");
        let keys: i64 =
            sqlx::query_scalar("select count(*) from idempotency_keys where user_id = $1")
                .bind(owner)
                .fetch_one(&p)
                .await?;
        assert_eq!(keys, if first_key.is_some() { 1 } else { 0 }, "{case}");
    }
    Ok(())
}

#[tokio::test]
async fn t042_failed_audit_rolls_back_create_and_allows_retry() -> TestResult {
    use gtt_control_plane::routes::tunnels::{create_tunnel, TunnelRouteConfig};

    let p = require_db!(pool().await);
    let owner = user_id(&p, &format!("t042-rollback-{}", Uuid::new_v4())).await?;
    let actor = policy::Actor::User {
        id: owner,
        status: UserStatus::Active,
    };
    let registry = gtt_profiles::Registry::load_dir(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../profiles"),
    )?;
    let config = TunnelRouteConfig::default();
    let body = r#"{"name":"Atomic","game_profile_id":"terraria","mappings":[{"mapping_key":"game","protocol":"tcp","enabled":true}]}"#;
    // PostgreSQL text rejects NUL. Fail the audit insert after tunnel/mapping insertion
    // without mutating the shared schema or interfering with parallel test fixtures.
    let failure = create_tunnel(
        &p,
        &registry,
        &config,
        actor,
        Some("retry-key"),
        body,
        "bad\0request",
    )
    .await
    .err()
    .ok_or("audit insertion should fail")?;
    assert_eq!(failure.code(), gtt_protocol::http::ApiErrorCode::Internal);
    assert_eq!(tunnels::count_live_tunnels(&p, owner).await?, 0);
    let mappings: i64 = sqlx::query_scalar("select count(*) from tunnel_mappings m join tunnels t on t.id = m.tunnel_id where t.owner_id = $1")
        .bind(owner).fetch_one(&p).await?;
    assert_eq!(mappings, 0);
    let keys: i64 = sqlx::query_scalar("select count(*) from idempotency_keys where user_id = $1")
        .bind(owner)
        .fetch_one(&p)
        .await?;
    assert_eq!(keys, 0);
    let retry = create_tunnel(
        &p,
        &registry,
        &config,
        actor,
        Some("retry-key"),
        body,
        "valid-request",
    )
    .await
    .map_err(|e| format!("retry failed: {e:?}"))?;
    assert!(!retry.replayed);
    assert_eq!(tunnels::count_live_tunnels(&p, owner).await?, 1);
    Ok(())
}

// ---------------------------------------------------------------------------
// T-043 -- start, stop, token routes and relay failover allocation (REL-002)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn t043_start_stop_token_and_failover_skip_unhealthy_relay() -> TestResult {
    let p = require_db!(pool().await);
    let owner = user_id(&p, "t043-owner").await?;
    let stranger = user_id(&p, "t043-stranger").await?;

    let profiles_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../profiles");
    let registry = gtt_profiles::Registry::load_dir(&profiles_dir)?;
    let route_cfg = gtt_control_plane::routes::tunnels::TunnelRouteConfig {
        max_live_tunnels: 5,
        tunnel_ttl_seconds: 3600,
        idempotency_ttl_seconds: 3600,
    };
    let start_cfg = gtt_control_plane::routes::tunnels::StartTunnelConfig::default();

    let kid = gtt_control_plane::keys::KeyId::new("kid-t043-db")?;
    let key_material = gtt_control_plane::keys::SigningKeyMaterial::from_bytes([0x55; 32])?;
    let key_ring = gtt_control_plane::keys::SigningKeyRing::new(
        gtt_control_plane::keys::DEFAULT_TOKEN_ISSUER,
        kid,
        key_material,
        Utc::now().timestamp(),
    );

    let owner_actor = policy::Actor::User {
        id: owner,
        status: UserStatus::Active,
    };
    let stranger_actor = policy::Actor::User {
        id: stranger,
        status: UserStatus::Active,
    };

    // Create an unhealthy relay and a healthy relay.
    let unhealthy_relay = relay::create_relay_node(
        &p,
        &format!("t043-unhealthy-{}", Uuid::new_v4()),
        "test-region",
        "unhealthy.relay.example.invalid",
        10,
        24000,
        24010,
    )
    .await?;
    relay::set_relay_status(&p, unhealthy_relay.id, RelayStatus::Unhealthy, false).await?;

    let healthy_relay = make_relay(&p, &format!("t043-healthy-{}", Uuid::new_v4())).await?;

    let create_body = r#"{"name":"Minecraft Java SMP","game_profile_id":"minecraft-java","mappings":[{"mapping_key":"game","protocol":"tcp","enabled":true},{"mapping_key":"rcon","protocol":"tcp","enabled":false}]}"#;
    let created = gtt_control_plane::routes::tunnels::create_tunnel(
        &p,
        &registry,
        &route_cfg,
        owner_actor,
        None,
        create_body,
        "req_t043_create",
    )
    .await
    .map_err(|e| format!("create_tunnel failed: {e:?}"))?;

    let start_idem = format!("start-idem-{}", Uuid::new_v4());

    // 1. Start allocates a healthy relay (skipping unhealthy_relay), reserves ports for enabled
    //    mappings only, transitions tunnel to active, and returns a schema-valid StartTunnelResponse.
    let started = gtt_control_plane::routes::tunnels::start_tunnel(
        &p,
        &key_ring,
        &start_cfg,
        owner_actor,
        &created.body.id,
        Some(&start_idem),
        "req_t043_start_1",
    )
    .await
    .map_err(|e| format!("start_tunnel failed: {e:?}"))?;
    assert_eq!(started.status, 200);
    assert!(!started.replayed);
    started.body.validate()?;
    assert_eq!(started.body.tunnel_id, created.body.id);
    assert_eq!(started.body.allocation.ports.len(), 1);
    assert_ne!(
        started.body.allocation.relay.host, "unhealthy.relay.example.invalid",
        "unhealthy relay must never be selected for a new allocation (REL-002)"
    );

    // 2. Repeating start with the same Idempotency-Key replays the same allocation.
    let replayed = gtt_control_plane::routes::tunnels::start_tunnel(
        &p,
        &key_ring,
        &start_cfg,
        owner_actor,
        &created.body.id,
        Some(&start_idem),
        "req_t043_start_2",
    )
    .await
    .map_err(|e| format!("start_tunnel replay failed: {e:?}"))?;
    assert_eq!(replayed.status, 200);
    assert!(replayed.replayed);
    assert_eq!(replayed.body.allocation.id, started.body.allocation.id);

    // 3. Route 10 (`POST /v1/tunnels/{id}/token`) mints a fresh signed token for the live allocation.
    let refreshed = gtt_control_plane::routes::tunnels::issue_tunnel_token(
        &p,
        &key_ring,
        &start_cfg,
        owner_actor,
        &created.body.id,
        "req_t043_token",
    )
    .await
    .map_err(|e| format!("issue_tunnel_token failed: {e:?}"))?;
    assert_eq!(refreshed.status, 200);
    refreshed.body.validate()?;

    // 4. Stranger calling start/token/stop gets 404 `not_found`, never `forbidden` (`SEC-009`).
    let stranger_stop = gtt_control_plane::routes::tunnels::stop_tunnel(
        &p,
        stranger_actor,
        &created.body.id,
        "req_t043_stranger_stop",
    )
    .await
    .err()
    .ok_or("stranger must not stop another user's tunnel")?;
    assert_eq!(stranger_stop.status, 404);
    assert_eq!(
        stranger_stop.code(),
        gtt_protocol::http::ApiErrorCode::NotFound
    );

    // 5. Failover reallocation (`REL-002`) moves the active tunnel off the previous relay onto a
    //    newly added healthy relay.
    let failover_relay = make_relay(&p, &format!("t043-failover-{}", Uuid::new_v4())).await?;
    relay::set_relay_status(&p, healthy_relay.id, RelayStatus::Draining, true).await?;

    let reallocated = gtt_control_plane::routes::tunnels::reallocate_tunnel_on_failover(
        &p,
        &key_ring,
        &start_cfg,
        owner_actor,
        &created.body.id,
        "req_t043_failover",
    )
    .await
    .map_err(|e| format!("reallocate_tunnel_on_failover failed: {e:?}"))?;
    assert_eq!(reallocated.status, 200);
    reallocated.body.validate()?;
    assert_ne!(
        reallocated.body.allocation.id, started.body.allocation.id,
        "failover reallocation must issue a new allocation after releasing the old one"
    );
    let _ = failover_relay;

    // 6. Stop releases the live allocation and ports, transitions tunnel to `stopped` (`INV-7`),
    //    and is naturally idempotent on repeat calls.
    let stopped = gtt_control_plane::routes::tunnels::stop_tunnel(
        &p,
        owner_actor,
        &created.body.id,
        "req_t043_stop_1",
    )
    .await
    .map_err(|e| format!("stop_tunnel failed: {e:?}"))?;
    assert_eq!(stopped.status, 200);
    assert!(!stopped.replayed);
    stopped.body.validate()?;
    assert_eq!(stopped.body.state, gtt_protocol::http::TunnelState::Stopped);
    assert!(stopped.body.stopped_at.is_some());

    let tunnel_uuid = Uuid::parse_str(&created.body.id)?;
    assert!(
        relay::get_live_allocation(&p, tunnel_uuid).await?.is_none(),
        "stopping a tunnel must release its live allocation"
    );

    // Repeat stop returns 200 idempotently.
    let stopped_again = gtt_control_plane::routes::tunnels::stop_tunnel(
        &p,
        owner_actor,
        &created.body.id,
        "req_t043_stop_2",
    )
    .await
    .map_err(|e| format!("idempotent stop_tunnel failed: {e:?}"))?;
    assert_eq!(stopped_again.status, 200);
    assert!(stopped_again.replayed);

    // Token issuance on a stopped tunnel returns 409 `conflict`.
    let token_after_stop = gtt_control_plane::routes::tunnels::issue_tunnel_token(
        &p,
        &key_ring,
        &start_cfg,
        owner_actor,
        &created.body.id,
        "req_t043_token_after_stop",
    )
    .await
    .err()
    .ok_or("expected 409 conflict when requesting token for stopped tunnel")?;
    assert_eq!(token_after_stop.status, 409);
    assert_eq!(
        token_after_stop.code(),
        gtt_protocol::http::ApiErrorCode::Conflict
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// T-045, T-046, T-048 -- quotas/suspension (SEC-008, SEC-015), audit leak check
//                        (SEC-013, PRIV-002), and relay state recovery (REL-004).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn t045_t046_t048_suspension_terminates_tunnels_audit_has_no_leaks_and_relay_state_recovers(
) -> TestResult {
    let p = require_db!(pool().await);
    let owner = user_id(&p, "t045-owner").await?;

    let profiles_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../profiles");
    let registry = gtt_profiles::Registry::load_dir(&profiles_dir)?;
    let route_cfg = gtt_control_plane::routes::tunnels::TunnelRouteConfig {
        max_live_tunnels: 5,
        tunnel_ttl_seconds: 3600,
        idempotency_ttl_seconds: 3600,
    };
    let start_cfg = gtt_control_plane::routes::tunnels::StartTunnelConfig::default();

    let kid = gtt_control_plane::keys::KeyId::new("kid-t048-db")?;
    let key_material = gtt_control_plane::keys::SigningKeyMaterial::from_bytes([0x66; 32])?;
    let key_ring = gtt_control_plane::keys::SigningKeyRing::new(
        gtt_control_plane::keys::DEFAULT_TOKEN_ISSUER,
        kid,
        key_material,
        Utc::now().timestamp(),
    );

    let owner_actor = policy::Actor::User {
        id: owner,
        status: UserStatus::Active,
    };
    let admin_actor = policy::Actor::Admin;

    let relay_a = make_relay(&p, &format!("t048-relay-a-{}", Uuid::new_v4())).await?;
    let relay_b = make_relay(&p, &format!("t048-relay-b-{}", Uuid::new_v4())).await?;

    // 1. Create and start a tunnel, and admit a guest.
    let create_body = r#"{"name":"T048 Recovery Tunnel","game_profile_id":"minecraft-java","mappings":[{"mapping_key":"game","protocol":"tcp","enabled":true}]}"#;
    let created = gtt_control_plane::routes::tunnels::create_tunnel(
        &p,
        &registry,
        &route_cfg,
        owner_actor,
        None,
        create_body,
        "req_t048_create",
    )
    .await
    .map_err(|e| format!("create_tunnel failed: {e:?}"))?;

    let _started = gtt_control_plane::routes::tunnels::start_tunnel(
        &p,
        &key_ring,
        &start_cfg,
        owner_actor,
        &created.body.id,
        None,
        "req_t048_start",
    )
    .await
    .map_err(|e| format!("start_tunnel failed: {e:?}"))?;

    let tunnel_uuid = Uuid::parse_str(&created.body.id)?;
    let alloc = relay::get_live_allocation(&p, tunnel_uuid)
        .await?
        .ok_or("expected live allocation after start")?;

    let inv = sharing::create_invitation(
        &p,
        tunnel_uuid,
        owner,
        b"hash-not-a-secret-t048",
        4,
        Utc::now() + chrono::Duration::hours(1),
    )
    .await?;
    sharing::admit_guest(
        &p,
        inv.id,
        tunnel_uuid,
        "198.51.100.88",
        Utc::now() + chrono::Duration::hours(1),
    )
    .await?;

    // 2. T-048 (`REL-004`): Simulated relay restart (`cursor = None`) restores live reservations
    //    and admissions for the owning relay, while another relay's credential gets 404 `not_found`.
    let relay_actor_on_alloc = policy::Actor::Relay { id: alloc.relay_id };
    let wrong_relay_id = if alloc.relay_id == relay_a.id {
        relay_b.id
    } else {
        relay_a.id
    };
    let wrong_relay_actor = policy::Actor::Relay { id: wrong_relay_id };

    let cross_relay_err = gtt_control_plane::routes::relay_internal::get_relay_state(
        &p,
        &key_ring,
        wrong_relay_actor,
        &alloc.relay_id.to_string(),
        None,
        "req_t048_wrong_relay",
    )
    .await
    .err()
    .ok_or("wrong relay must be denied")?;
    assert_eq!(cross_relay_err.status, 404);
    assert_eq!(
        cross_relay_err.code(),
        gtt_protocol::http::ApiErrorCode::NotFound
    );

    let state_resp = gtt_control_plane::routes::relay_internal::get_relay_state(
        &p,
        &key_ring,
        relay_actor_on_alloc,
        &alloc.relay_id.to_string(),
        None,
        "req_t048_state_full",
    )
    .await
    .map_err(|e| format!("get_relay_state failed: {e:?}"))?;
    assert_eq!(state_resp.status, 200);
    assert!(
        state_resp
            .body
            .reservations
            .iter()
            .any(|a| a.allocation_id == alloc.id.to_string()),
        "relay restart recovery must restore the live allocation"
    );
    assert!(
        state_resp
            .body
            .admissions
            .iter()
            .any(|a| a.tunnel_id == tunnel_uuid.to_string() && a.source_ip == "198.51.100.88"),
        "relay restart recovery must restore the live guest admission"
    );

    // 3. T-048 Route 20 (`POST /v1/internal/relays/{id}/report`) activates the allocation and
    //    records usage deltas.
    let report_body = serde_json::json!({
        "active_allocations": 1,
        "active_flows": 2,
        "activated_allocations": [alloc.id.to_string()],
        "usage": [{
            "tunnel_id": tunnel_uuid.to_string(),
            "interval_start": Utc::now().to_rfc3339(),
            "bytes_from_guests": 4096,
            "bytes_to_guests": 8192,
            "peak_flows": 2,
            "datagrams_dropped": 0
        }]
    })
    .to_string();
    let report_resp = gtt_control_plane::routes::relay_internal::report_relay_status_and_usage(
        &p,
        relay_actor_on_alloc,
        &alloc.relay_id.to_string(),
        &report_body,
        "req_t048_report",
    )
    .await
    .map_err(|e| format!("report_relay_status_and_usage failed: {e:?}"))?;
    assert_eq!(report_resp.status, 204);

    let after_report_alloc = relay::get_live_allocation(&p, tunnel_uuid)
        .await?
        .ok_or("allocation should still be live after report")?;
    assert_eq!(after_report_alloc.state, "active");

    // 4. T-045 (`SEC-015`): Admin suspends the user; all active tunnels are terminated, live
    //    allocations/ports are released, sessions are revoked, and an audit event is recorded.
    let changed = gtt_control_plane::limiter::suspend_user_account(
        &p,
        admin_actor,
        owner,
        Some("abuse_rate_exceeded"),
        "req_t045_suspend",
    )
    .await
    .map_err(|e| format!("suspend_user_account failed: {e:?}"))?;
    assert_eq!(changed, 1);

    let tunnel_after_suspend = tunnels::get_tunnel(&p, owner, tunnel_uuid)
        .await?
        .ok_or("tunnel disappeared")?;
    assert_eq!(tunnel_after_suspend.state, "terminated");
    assert!(
        relay::get_live_allocation(&p, tunnel_uuid).await?.is_none(),
        "suspending a user must release live relay allocations on their terminated tunnels"
    );

    // 5. T-046 (`SEC-013`, `PRIV-002`): Verify audit events exist for tunnel creation, start, and
    //    suspension, and scan all audit metadata for sensitive leaks.
    let tunnel_events = ops::list_audit_events(&p, "tunnel", Some(tunnel_uuid), 50).await?;
    assert!(
        tunnel_events.iter().any(|e| e.action == "tunnel.created"),
        "expected tunnel.created audit event"
    );
    assert!(
        tunnel_events.iter().any(|e| e.action == "tunnel.started"),
        "expected tunnel.started audit event"
    );
    let user_events = ops::list_audit_events(&p, "user", Some(owner), 50).await?;
    assert!(
        user_events
            .iter()
            .any(|e| e.action == "admin.user_suspended"),
        "expected admin.user_suspended audit event"
    );
    for ev in tunnel_events.iter().chain(user_events.iter()) {
        let serialized = serde_json::to_string(&ev.metadata)?;
        assert!(
            gtt_control_plane::redaction::scan_for_sensitive_leaks(&serialized).is_empty(),
            "audit metadata leaked sensitive material: {serialized}"
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// T-044 -- Invitations, claim, and admissions (FR-007, FR-008, FR-009, SEC-007)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn t044_invitations_claim_admissions_and_revocation() -> TestResult {
    let p = require_db!(pool().await);
    let owner = user_id(&p, "t044-owner").await?;
    let stranger = user_id(&p, "t044-stranger").await?;

    let profiles_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../profiles");
    let registry = gtt_profiles::Registry::load_dir(&profiles_dir)?;
    let route_cfg = gtt_control_plane::routes::tunnels::TunnelRouteConfig {
        max_live_tunnels: 5,
        tunnel_ttl_seconds: 3600,
        idempotency_ttl_seconds: 3600,
    };
    let start_cfg = gtt_control_plane::routes::tunnels::StartTunnelConfig::default();
    let inv_cfg = gtt_control_plane::routes::invitations::InvitationRouteConfig::default();

    let kid = gtt_control_plane::keys::KeyId::new("kid-t044-db")?;
    let key_material = gtt_control_plane::keys::SigningKeyMaterial::from_bytes([0x77; 32])?;
    let key_ring = gtt_control_plane::keys::SigningKeyRing::new(
        gtt_control_plane::keys::DEFAULT_TOKEN_ISSUER,
        kid,
        key_material,
        Utc::now().timestamp(),
    );

    let owner_actor = policy::Actor::User {
        id: owner,
        status: UserStatus::Active,
    };
    let stranger_actor = policy::Actor::User {
        id: stranger,
        status: UserStatus::Active,
    };

    let _relay = make_relay(&p, &format!("t044-relay-{}", Uuid::new_v4())).await?;

    let create_body = r#"{"name":"T044 Minecraft Server","game_profile_id":"minecraft-java","mappings":[{"mapping_key":"game","protocol":"tcp","enabled":true}]}"#;
    let created = gtt_control_plane::routes::tunnels::create_tunnel(
        &p,
        &registry,
        &route_cfg,
        owner_actor,
        None,
        create_body,
        "req_t044_create_tunnel",
    )
    .await
    .map_err(|e| format!("create_tunnel failed: {e:?}"))?;

    let _started = gtt_control_plane::routes::tunnels::start_tunnel(
        &p,
        &key_ring,
        &start_cfg,
        owner_actor,
        &created.body.id,
        None,
        "req_t044_start_tunnel",
    )
    .await
    .map_err(|e| format!("start_tunnel failed: {e:?}"))?;

    // 1. Route 12 (`POST /v1/tunnels/{id}/invitations`) with `max_claims = 1` returns `201` and a
    //    one-time secret (`SEC-007`).
    let inv_created = gtt_control_plane::routes::invitations::create_invitation(
        &p,
        &inv_cfg,
        owner_actor,
        &created.body.id,
        r#"{"expires_in_seconds":3600,"max_claims":1}"#,
        "req_t044_create_inv",
    )
    .await
    .map_err(|e| format!("create_invitation failed: {e:?}"))?;
    assert_eq!(inv_created.status, 201);
    inv_created.body.validate()?;
    let secret = inv_created.body.secret.clone();

    // 2. Route 13 (`GET /v1/tunnels/{id}/invitations`) never returns the plaintext secret.
    let listed = gtt_control_plane::routes::invitations::list_invitations(
        &p,
        owner_actor,
        &created.body.id,
        "req_t044_list_inv",
    )
    .await
    .map_err(|e| format!("list_invitations failed: {e:?}"))?;
    assert_eq!(listed.status, 200);
    let listed_json = serde_json::to_string(&listed.body)?;
    assert!(
        !listed_json.contains(&secret),
        "list_invitations must never return the invitation secret"
    );

    // 3. Route 18 (`POST /v1/invitations/claim`) succeeds on the first claim, admits the guest IP,
    //    and returns `422 invitation_invalid` when exhausted on the second claim (`FR-007`, `FR-008`).
    let claim_body = format!(r#"{{"secret":"{secret}"}}"#);
    let claimed = gtt_control_plane::routes::invitations::claim_invitation(
        &p,
        &inv_cfg,
        &claim_body,
        "198.51.100.44",
        "req_t044_claim_1",
    )
    .await
    .map_err(|e| format!("claim_invitation failed: {e:?}"))?;
    assert_eq!(claimed.status, 200);
    claimed.body.validate()?;

    let exhausted_err = gtt_control_plane::routes::invitations::claim_invitation(
        &p,
        &inv_cfg,
        &claim_body,
        "198.51.100.45",
        "req_t044_claim_exhausted",
    )
    .await
    .err()
    .ok_or("expected 422 invitation_invalid on exhausted invitation")?;
    assert_eq!(exhausted_err.status, 422);
    assert_eq!(
        exhausted_err.code(),
        gtt_protocol::http::ApiErrorCode::InvitationInvalid
    );

    // 4. Route 15 (`GET /v1/tunnels/{id}/admissions`) returns masked source IP (`198.51.100.0/24`),
    //    never the raw guest IP (`198.51.100.44`, `PRIV-002`).
    let admissions = gtt_control_plane::routes::invitations::list_admissions(
        &p,
        owner_actor,
        &created.body.id,
        "req_t044_list_adm",
    )
    .await
    .map_err(|e| format!("list_admissions failed: {e:?}"))?;
    assert_eq!(admissions.status, 200);
    assert_eq!(admissions.body.len(), 1);
    let adm0 = admissions.body.first().ok_or("missing admission")?;
    assert_eq!(adm0.masked_source_ip, "198.51.100.0/24");

    // 5. Stranger gets `404 not_found` when attempting to revoke the admission or invitation (`SEC-009`).
    let stranger_revoke = gtt_control_plane::routes::invitations::revoke_admission(
        &p,
        stranger_actor,
        &adm0.id,
        "req_t044_stranger_revoke",
    )
    .await
    .err()
    .ok_or("stranger must not revoke admission")?;
    assert_eq!(stranger_revoke.status, 404);

    // 6. Owner revokes a second invitation and verifies claiming it returns `422 invitation_invalid`
    //    (`FR-007`, `FR-009`).
    let inv2 = gtt_control_plane::routes::invitations::create_invitation(
        &p,
        &inv_cfg,
        owner_actor,
        &created.body.id,
        r#"{"expires_in_seconds":3600,"max_claims":5}"#,
        "req_t044_create_inv2",
    )
    .await
    .map_err(|e| format!("create_invitation 2 failed: {e:?}"))?;
    let revoked_resp = gtt_control_plane::routes::invitations::revoke_invitation(
        &p,
        owner_actor,
        &inv2.body.id,
        "req_t044_revoke_inv2",
    )
    .await
    .map_err(|e| format!("revoke_invitation failed: {e:?}"))?;
    assert_eq!(revoked_resp.status, 204);

    let claim_revoked_body = format!(r#"{{"secret":"{}"}}"#, inv2.body.secret);
    let revoked_err = gtt_control_plane::routes::invitations::claim_invitation(
        &p,
        &inv_cfg,
        &claim_revoked_body,
        "198.51.100.46",
        "req_t044_claim_revoked",
    )
    .await
    .err()
    .ok_or("expected 422 invitation_invalid on revoked invitation")?;
    assert_eq!(revoked_err.status, 422);
    assert_eq!(
        revoked_err.code(),
        gtt_protocol::http::ApiErrorCode::InvitationInvalid
    );

    Ok(())
}

#[tokio::test]
async fn t109_retention_cleanup_sweep_purges_expired_rows_across_all_categories() -> TestResult {
    use gtt_control_plane::db::cleanup::{run_retention_sweep, RetentionPolicyConfig};

    let p = require_db!(pool().await);
    let owner = user_id(&p, "retention-owner").await?;
    let relay = make_relay(&p, "retention-relay").await?;

    // 1. Setup live and expired tunnels (PRIV-003, T-109).
    let live_tunnel_id = make_tunnel(&p, "retention-owner", "retention-live").await?;
    let expired_tunnel_id: Uuid = sqlx::query_scalar(
        "insert into tunnels
             (owner_id, name, game_profile_id, profile_version, state, created_at,
              started_at, stopped_at, expires_at)
         values ($1, 'retention-terminal-past', 'minecraft-java', 1, 'stopped',
                 now() - interval '50 days', now() - interval '50 days',
                 now() - interval '40 days', now() + interval '1 day')
         returning id",
    )
    .bind(owner)
    .fetch_one(&p)
    .await?;

    // 2. Setup auth sessions: 1 active, 1 expired beyond 7-day retention.
    let token_hash_live = [0x11u8; 32];
    let token_hash_exp = [0x22u8; 32];
    let live_session = accounts::create_session(
        &p,
        owner,
        Uuid::new_v4(),
        &token_hash_live,
        Some("live-device"),
        Utc::now() + chrono::Duration::days(14),
    )
    .await?;
    let expired_session_id: Uuid = sqlx::query_scalar(
        "insert into auth_sessions
             (user_id, family_id, refresh_token_hash, device_label, created_at, expires_at)
         values ($1, $2, $3, 'expired-device', now() - interval '20 days', now() - interval '10 days')
         returning id",
    )
    .bind(owner)
    .bind(Uuid::new_v4())
    .bind(&token_hash_exp[..])
    .fetch_one(&p)
    .await?;

    // 3. Setup invitations & guest admissions on live tunnel:
    // 1 active admission, 1 expired admission, 1 revoked admission past 24h retention.
    let inv_id: Uuid = sqlx::query_scalar(
        "insert into invitations (tunnel_id, created_by, token_hash, max_claims, expires_at)
         values ($1, $2, $3, 10, now() + interval '7 days')
         returning id",
    )
    .bind(live_tunnel_id)
    .bind(owner)
    .bind(&[0x33u8; 32][..])
    .fetch_one(&p)
    .await?;

    let live_adm_id: Uuid = sqlx::query_scalar(
        "insert into guest_admissions (invitation_id, tunnel_id, source_ip, expires_at)
         values ($1, $2, '198.51.100.11'::inet, now() + interval '2 days')
         returning id",
    )
    .bind(inv_id)
    .bind(live_tunnel_id)
    .fetch_one(&p)
    .await?;

    let expired_adm_id: Uuid = sqlx::query_scalar(
        "insert into guest_admissions (invitation_id, tunnel_id, source_ip, created_at, expires_at)
         values ($1, $2, '198.51.100.12'::inet, now() - interval '5 days', now() - interval '3 days')
         returning id",
    )
    .bind(inv_id)
    .bind(live_tunnel_id)
    .fetch_one(&p)
    .await?;

    let revoked_adm_id: Uuid = sqlx::query_scalar(
        "insert into guest_admissions (invitation_id, tunnel_id, source_ip, created_at, expires_at, revoked_at)
         values ($1, $2, '198.51.100.13'::inet, now() - interval '5 days', now() + interval '1 day', now() - interval '2 days')
         returning id",
    )
    .bind(inv_id)
    .bind(live_tunnel_id)
    .fetch_one(&p)
    .await?;

    // 4. Setup idempotency keys: 1 fresh, 1 expired past 24h retention.
    sqlx::query(
        "insert into idempotency_keys (user_id, route, idem_key, request_hash, expires_at)
         values ($1, '/v1/tunnels', 'idem_retention_live', $2, now() + interval '1 day')",
    )
    .bind(owner)
    .bind(&[0x44u8; 32][..])
    .execute(&p)
    .await?;

    sqlx::query(
        "insert into idempotency_keys (user_id, route, idem_key, request_hash, created_at, expires_at)
         values ($1, '/v1/tunnels', 'idem_retention_exp', $2, now() - interval '5 days', now() - interval '3 days')",
    )
    .bind(owner)
    .bind(&[0x55u8; 32][..])
    .execute(&p)
    .await?;

    // 5. Setup usage records: 1 recent, 1 past 90-day retention on live tunnel.
    sqlx::query(
        "insert into usage_records (tunnel_id, relay_id, interval_start, bytes_from_guests, bytes_to_guests)
         values ($1, $2, now(), 1000, 2000)",
    )
    .bind(live_tunnel_id)
    .bind(relay.id)
    .execute(&p)
    .await?;

    sqlx::query(
        "insert into usage_records (tunnel_id, relay_id, interval_start, bytes_from_guests, bytes_to_guests)
         values ($1, $2, now() - interval '120 days', 5000, 10000)",
    )
    .bind(live_tunnel_id)
    .bind(relay.id)
    .execute(&p)
    .await?;

    // 6. Setup audit events: 1 recent, 1 past 90-day retention.
    sqlx::query(
        "insert into audit_events (occurred_at, actor_type, actor_id, action, subject_type, subject_id, metadata)
         values (now(), 'user', $1, 'tunnel.create', 'tunnel', $2, '{}'::jsonb)",
    )
    .bind(owner)
    .bind(live_tunnel_id)
    .execute(&p)
    .await?;

    sqlx::query(
        "insert into audit_events (occurred_at, actor_type, actor_id, action, subject_type, subject_id, metadata)
         values (now() - interval '100 days', 'user', $1, 'tunnel.old', 'tunnel', $2, '{}'::jsonb)",
    )
    .bind(owner)
    .bind(live_tunnel_id)
    .execute(&p)
    .await?;

    // 7. Setup soft-deleted user past 30-day retention.
    let deleted_user_id: Uuid = sqlx::query_scalar(
        "insert into users (idp_issuer, idp_subject, display_name, status, created_at, deleted_at)
         values ('retention-del-issuer', 'retention-del-sub', 'Deleted User', 'active', now() - interval '60 days', now() - interval '35 days')
         returning id",
    )
    .fetch_one(&p)
    .await?;

    // 8. Execute the retention sweep (T-109, PRIV-003, PRIV-004).
    let sweep_report =
        run_retention_sweep(&p, &RetentionPolicyConfig::default(), Utc::now(), 100).await?;

    // Assert that expired/retained rows were purged.
    assert!(sweep_report.purged_guest_admissions >= 2);
    assert!(sweep_report.purged_idempotency_keys >= 1);
    assert!(sweep_report.purged_auth_sessions >= 1);
    assert!(sweep_report.purged_terminal_tunnels >= 1);
    assert!(sweep_report.purged_usage_records >= 1);
    assert!(sweep_report.purged_audit_events >= 1);
    assert!(sweep_report.purged_soft_deleted_users >= 1);

    // Assert that active / unexpired entities remain intact.
    let live_tunnel_exists: bool =
        sqlx::query_scalar("select exists(select 1 from tunnels where id = $1)")
            .bind(live_tunnel_id)
            .fetch_one(&p)
            .await?;
    assert!(live_tunnel_exists, "live tunnel must remain");

    let expired_tunnel_exists: bool =
        sqlx::query_scalar("select exists(select 1 from tunnels where id = $1)")
            .bind(expired_tunnel_id)
            .fetch_one(&p)
            .await?;
    assert!(
        !expired_tunnel_exists,
        "expired terminal tunnel must be purged"
    );

    let live_adm_exists: bool =
        sqlx::query_scalar("select exists(select 1 from guest_admissions where id = $1)")
            .bind(live_adm_id)
            .fetch_one(&p)
            .await?;
    assert!(live_adm_exists, "live admission must remain");

    let expired_adm_exists: bool =
        sqlx::query_scalar("select exists(select 1 from guest_admissions where id = $1)")
            .bind(expired_adm_id)
            .fetch_one(&p)
            .await?;
    assert!(!expired_adm_exists, "expired admission must be purged");

    let revoked_adm_exists: bool =
        sqlx::query_scalar("select exists(select 1 from guest_admissions where id = $1)")
            .bind(revoked_adm_id)
            .fetch_one(&p)
            .await?;
    assert!(
        !revoked_adm_exists,
        "revoked admission past retention must be purged"
    );

    let live_session_exists: bool =
        sqlx::query_scalar("select exists(select 1 from auth_sessions where id = $1)")
            .bind(live_session.id)
            .fetch_one(&p)
            .await?;
    assert!(live_session_exists, "live auth session must remain");

    let expired_session_exists: bool =
        sqlx::query_scalar("select exists(select 1 from auth_sessions where id = $1)")
            .bind(expired_session_id)
            .fetch_one(&p)
            .await?;
    assert!(
        !expired_session_exists,
        "expired auth session must be purged"
    );

    let deleted_user_exists: bool =
        sqlx::query_scalar("select exists(select 1 from users where id = $1)")
            .bind(deleted_user_id)
            .fetch_one(&p)
            .await?;
    assert!(
        !deleted_user_exists,
        "soft-deleted user past 30 days must be purged"
    );

    Ok(())
}

#[tokio::test]
async fn t098_account_deletion_immediate_termination_and_cascading_purge() -> TestResult {
    let p = require_db!(pool().await);

    // 1. Create a user, tunnel with mappings, reservation, invitation, admission, and auth session.
    let owner_handle = "t098-del-owner";
    let tunnel_id = make_tunnel(&p, owner_handle, "t098-del-tunnel").await?;
    let owner = user_id(&p, owner_handle).await?;
    let node = make_relay(&p, "t098-del-relay").await?;
    let expires = Utc::now() + chrono::Duration::hours(1);

    // Mark tunnel active
    sqlx::query("update tunnels set state = 'active', started_at = now() where id = $1")
        .bind(tunnel_id)
        .execute(&p)
        .await?;

    let alloc = relay::reserve_allocation(&p, tunnel_id, node.id, expires).await?;

    let token_hash = [0x55u8; 32];
    let session_id: Uuid = sqlx::query_scalar(
        "insert into auth_sessions (user_id, refresh_token_hash, expires_at)
         values ($1, $2, now() + interval '7 days')
         returning id",
    )
    .bind(owner)
    .bind(&token_hash[..])
    .fetch_one(&p)
    .await?;

    let inv_id: Uuid = sqlx::query_scalar(
        "insert into invitations (tunnel_id, created_by, token_hash, max_claims, expires_at)
         values ($1, $2, $3, 10, now() + interval '7 days')
         returning id",
    )
    .bind(tunnel_id)
    .bind(owner)
    .bind(&token_hash[..])
    .fetch_one(&p)
    .await?;

    let adm_id: Uuid = sqlx::query_scalar(
        "insert into guest_admissions (invitation_id, tunnel_id, source_ip, expires_at)
         values ($1, $2, '198.51.100.22'::inet, now() + interval '2 days')
         returning id",
    )
    .bind(inv_id)
    .bind(tunnel_id)
    .fetch_one(&p)
    .await?;

    // 2. Soft-delete the user (PRIV-004)
    let affected = accounts::soft_delete_user(&p, owner).await?;
    assert_eq!(affected, 1, "soft-deleted rows affected must be exactly 1");

    // Verify immediate consequences (PRIV-004):
    // a. User is invisible to standard lookups
    let fetched_user = accounts::get_user(&p, owner).await?;
    assert!(
        fetched_user.is_none(),
        "soft-deleted user must not be returned by get_user"
    );

    // b. Active tunnels immediately terminate with reason 'account deleted'
    let tunnel_row = tunnels::get_tunnel(&p, owner, tunnel_id)
        .await?
        .ok_or("tunnel disappeared")?;
    assert_eq!(tunnel_row.state, "terminated");
    assert_eq!(
        tunnel_row.termination_reason.as_deref(),
        Some("account deleted")
    );
    assert!(
        tunnel_row.stopped_at.is_some(),
        "stopped_at must be populated immediately"
    );

    // c. Relay allocation is immediately released
    let alloc_state: String =
        sqlx::query_scalar("select state from relay_allocations where id = $1")
            .bind(alloc.id)
            .fetch_one(&p)
            .await?;
    assert_eq!(
        alloc_state, "released",
        "relay allocation must be released immediately"
    );

    // d. Auth sessions are immediately revoked
    let session_revoked: bool =
        sqlx::query_scalar("select revoked_at is not null from auth_sessions where id = $1")
            .bind(session_id)
            .fetch_one(&p)
            .await?;
    assert!(session_revoked, "auth session must be revoked immediately");

    // 3. Fast-forward user deletion past 30-day retention schedule (PRIV-003, PRIV-004)
    sqlx::query("update users set deleted_at = now() - interval '31 days' where id = $1")
        .bind(owner)
        .execute(&p)
        .await?;

    // 4. Run retention cleanup purge
    let purged = gtt_control_plane::db::cleanup::purge_soft_deleted_users(
        &p,
        Utc::now() - chrono::Duration::days(30),
        100,
    )
    .await?;
    assert!(purged >= 1, "purge count must include soft-deleted user");

    // 5. Verify cascading purge: user and all dependent rows are completely gone (SEC-009, PRIV-004)
    let user_exists: bool = sqlx::query_scalar("select exists(select 1 from users where id = $1)")
        .bind(owner)
        .fetch_one(&p)
        .await?;
    assert!(!user_exists, "user record must be purged from database");

    let tunnel_exists: bool =
        sqlx::query_scalar("select exists(select 1 from tunnels where id = $1)")
            .bind(tunnel_id)
            .fetch_one(&p)
            .await?;
    assert!(!tunnel_exists, "tunnel record must be cascade-purged");

    let session_exists: bool =
        sqlx::query_scalar("select exists(select 1 from auth_sessions where id = $1)")
            .bind(session_id)
            .fetch_one(&p)
            .await?;
    assert!(!session_exists, "auth session must be cascade-purged");

    let inv_exists: bool =
        sqlx::query_scalar("select exists(select 1 from invitations where id = $1)")
            .bind(inv_id)
            .fetch_one(&p)
            .await?;
    assert!(!inv_exists, "invitation must be cascade-purged");

    let adm_exists: bool =
        sqlx::query_scalar("select exists(select 1 from guest_admissions where id = $1)")
            .bind(adm_id)
            .fetch_one(&p)
            .await?;
    assert!(!adm_exists, "guest admission must be cascade-purged");

    Ok(())
}
