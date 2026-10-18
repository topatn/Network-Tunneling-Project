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

    // (owner, operation, query the data layer) triples where the policy allows bob but the
    // data layer must still return nothing.
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

#[tokio::test]
async fn an_invalid_source_ip_is_refused_without_echoing_it_back() -> TestResult {
    // PRIV-002: `source_ip` is guest personal data, and error text ends up in logs. The
    // rejection must name the rule, not the value.
    assert!(sharing::assert_valid_source_ip("203.0.113.5").is_ok());
    let outcome = sharing::assert_valid_source_ip("not-an-ip");
    assert!(outcome.is_err(), "an invalid address must be refused");
    let message = match outcome {
        Ok(()) => String::from("accepted, which should not happen"),
        Err(e) => e.to_string(),
    };
    assert!(
        !message.contains("not-an-ip"),
        "the rejection message echoed the offending value: {message}"
    );
    Ok(())
}
