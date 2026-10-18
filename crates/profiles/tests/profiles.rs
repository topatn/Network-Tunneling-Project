//! `GAME-007` and `GAME-008`: profile loading, validation, and default-enabled mappings.
//!
//! ## What each of T-032's Done-when clauses is checked by
//!
//! * *"Invalid profile is rejected"* — [`invalid_profiles_are_rejected_with_a_reason`],
//!   [`structural_faults_are_rejected_by_the_types`], and the bounds tests.
//! * *"Each game's default-enabled mappings match its profile"* —
//!   [`shipped_profiles_expose_only_their_default_enabled_mappings`], which reads the files
//!   in `profiles/` rather than a copy of them, so editing a file without updating an
//!   expectation fails here.
//! * *"A sixth sample profile loads with no code change"* —
//!   [`a_sixth_profile_loads_with_no_code_change`].
//!
//! ## Why several tests build a temporary profile file rather than calling the validator
//!
//! A validator test proves the checks run. It does not prove the **deserializer** accepts or
//! rejects what a file actually contains, and the deserializer is where unknown fields, bad
//! protocol names and out-of-range ports are actually caught. Those faults cannot be produced
//! by constructing a `Profile` in Rust at all — `Profile { mappings: vec![] }` is a value, not
//! a file — so a test that never writes a file is asserting less than it appears to.
//!
//! ## No `expect` and no `panic!`
//!
//! The workspace denies the `panic` lint group on **all** targets, tests included, so every
//! test returns `TestResult` and propagates with `?`. A failure prints the `Debug` form of the
//! underlying error, which for this crate is more informative than a message written by hand —
//! `DuplicateId` names both offending files, `TooLarge` names the size and the limit.

use std::fs;
use std::path::{Path, PathBuf};

use gtt_profiles::{is_valid_identifier, Evidence, Mapping, Protocol, Registry, RegistryError};

/// Tests propagate rather than panic, because the lint group forbids `expect` and `panic!`.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The five games the product ships, with the mappings each must expose by default.
///
/// Written out longhand rather than derived from the files, because the point of
/// [`shipped_profiles_expose_only_their_default_enabled_mappings`] is to catch an edit to a
/// profile that nobody re-derived this table for. Deriving it from the files would make the
/// test vacuous.
const EXPECTED_DEFAULTS: &[(&str, &[&str])] = &[
    // Minecraft Java: query and RCON are optional, so only the game port is exposed.
    // research.md RQ-1 notes `server-port` and `query.port` share 25565 on different
    // protocols, which is why both can exist as separate mappings.
    ("minecraft-java", &["game"]),
    // Bedrock: the IPv6 port is optional.
    ("minecraft-bedrock", &["game"]),
    // Terraria: TCP is verified. The UDP mapping is `[R]` and optional, so off twice over.
    ("terraria", &["game"]),
    // L4D2: the game port only. RCON shares 27015 on TCP; SourceTV and client are optional.
    ("l4d2", &["game"]),
    // L4D1: the one mapping is `[R]`, so nothing is exposed by default. See the crate docs
    // on `Evidence` -- an unverified game is opt-in, not excluded.
    ("l4d1", &[]),
];

/// A directory holding scratch profile files, removed when dropped.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    /// Returns the value, not `TestResult`: a `Result<(), _>` here would make every call site
    /// infer `()` and then fail to find `write` on it.
    fn new(label: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let dir =
            std::env::temp_dir().join(format!("gtt-profile-test-{label}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    fn write(&self, name: &str, body: &str) -> TestResult {
        fs::write(self.dir.join(name), body)?;
        Ok(())
    }

    fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort. A leftover directory in temp is harmless, and a failure here would mask
        // the test's real result, so it is deliberately not propagated.
        drop(fs::remove_dir_all(&self.dir));
    }
}

/// A minimal valid profile, for cases that vary one field.
fn minimal_json() -> String {
    r#"{
      "id": "sample-game",
      "version": 1,
      "display_name": "Sample Game",
      "mappings": [
        { "key": "game", "protocol": "tcp", "default_port": 1234, "optional": false, "evidence": "verified" }
      ]
    }"#
    .to_string()
}

/// A profile for a game nothing in this source has heard of.
fn sixth_profile_json() -> String {
    r#"{
      "id": "obsidian-drift",
      "version": 3,
      "display_name": "Obsidian Drift",
      "mappings": [
        { "key": "game", "protocol": "udp", "default_port": 7777, "optional": false, "evidence": "verified" },
        { "key": "spectator", "protocol": "udp", "default_port": 7000, "optional": true, "evidence": "verified" }
      ]
    }"#
    .to_string()
}

/// `profiles/` at the repository root.
///
/// `CARGO_MANIFEST_DIR` is `crates/profiles`, so the root is two levels up. Used instead of a
/// relative path so the test does not depend on the working directory the runner chose.
fn repo_profiles_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("profiles")
}

// ---------------------------------------------------------------------------
// The shipped profiles
// ---------------------------------------------------------------------------

#[test]
fn shipped_profiles_expose_only_their_default_enabled_mappings() -> TestResult {
    let registry = Registry::load_dir(&repo_profiles_dir())?;
    assert_eq!(
        registry.len(),
        EXPECTED_DEFAULTS.len(),
        "the number of shipped profiles changed; update EXPECTED_DEFAULTS and say why"
    );

    for (id, expected_keys) in EXPECTED_DEFAULTS {
        let Some(profile) = registry.get(id) else {
            return Err(format!("no profile with id {id:?}").into());
        };
        assert_eq!(
            profile.default_enabled_keys(),
            expected_keys.to_vec(),
            "{id}: GAME-008 says a new tunnel exposes only the default-enabled mappings"
        );
    }
    Ok(())
}

#[test]
fn every_shipped_profile_has_at_least_one_non_optional_mapping() -> TestResult {
    // Deliberately not "at least one *enabled* mapping": L4D1's only mapping is non-optional
    // and unverified, which is a real state. What must hold is that no profile describes a
    // game with nothing to route.
    let registry = Registry::load_dir(&repo_profiles_dir())?;
    for profile in registry.profiles() {
        assert!(
            profile.mappings.iter().any(|m| !m.optional),
            "{}: no non-optional mapping, so the game could not be hosted at all",
            profile.id
        );
    }
    Ok(())
}

#[test]
fn shipped_profiles_declare_a_port_and_a_protocol_and_nothing_else() -> TestResult {
    // INV-1: a profile carries a default *port* per mapping and never a destination. The
    // structural guarantee is `deny_unknown_fields`; this confirms the shipped files do not
    // lean on a field that does not exist.
    let registry = Registry::load_dir(&repo_profiles_dir())?;
    for profile in registry.profiles() {
        for m in &profile.mappings {
            assert!(m.default_port > 0, "{}:{} has port 0", profile.id, m.key);
            assert!(
                matches!(m.protocol, Protocol::Tcp | Protocol::Udp),
                "{}:{}",
                profile.id,
                m.key
            );
        }
    }
    Ok(())
}

#[test]
fn mapping_keys_satisfy_the_wire_rule_contracts_md_sets() -> TestResult {
    // A key valid in a profile must be valid on the wire without translation, so both use the
    // same rule. `contracts.md` §5.6: `^[a-z][a-z0-9-]{0,31}$`.
    let registry = Registry::load_dir(&repo_profiles_dir())?;
    for profile in registry.profiles() {
        assert!(is_valid_identifier(&profile.id), "{}", profile.id);
        for m in &profile.mappings {
            assert!(
                is_valid_identifier(&m.key),
                "{}: {:?} breaks the wire rule",
                profile.id,
                m.key
            );
        }
    }
    Ok(())
}

#[test]
fn profiles_load_in_a_deterministic_order() -> TestResult {
    // Two machines disagreeing about directory read order must not disagree about which file
    // a load failure names, so the order is sorted by filename.
    let a = Registry::load_dir(&repo_profiles_dir())?;
    let b = Registry::load_dir(&repo_profiles_dir())?;
    let ids = a.ids().collect::<Vec<_>>();
    assert_eq!(ids, b.ids().collect::<Vec<_>>(), "order is not stable");

    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted, "profiles should be in filename order");
    Ok(())
}

#[test]
fn lookup_distinguishes_a_game_from_a_version_a_tunnel_recorded() -> TestResult {
    let registry = Registry::load_dir(&repo_profiles_dir())?;
    let id = "minecraft-java";

    assert!(registry.get(id).is_some(), "the game exists");
    assert!(registry.get("no-such-game").is_none());

    let Some(current) = registry.get(id).map(|p| p.version) else {
        return Err("minecraft-java is missing".into());
    };
    assert!(
        registry.get_versioned(id, current).is_some(),
        "the version a live tunnel records must still resolve"
    );
    assert!(
        registry.get_versioned(id, current + 1).is_none(),
        "a tunnel created under a different profile version must not silently match"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// GAME-007: a sixth profile needs no code change
// ---------------------------------------------------------------------------

#[test]
fn a_sixth_profile_loads_with_no_code_change() -> TestResult {
    // GAME-007's verification, literally: a game this source has never heard of, in a
    // directory, loaded through the ordinary path. If any game were named in the loader this
    // test could not pass without editing the loader too.
    let scratch = Scratch::new("sixth")?;
    scratch.write("z-obsidian-drift.json", &sixth_profile_json())?;

    let registry = Registry::load_dir(scratch.dir())?;
    assert_eq!(registry.len(), 1);

    let Some(profile) = registry.get("obsidian-drift") else {
        return Err("the new profile did not load".into());
    };
    assert_eq!(profile.version, 3);
    assert_eq!(profile.display_name, "Obsidian Drift");
    assert_eq!(profile.default_enabled_keys(), vec!["game"]);

    // Its optional mapping is present and loadable, just not exposed.
    let Some(optional) = profile.mapping("spectator") else {
        return Err("the optional mapping is missing".into());
    };
    assert!(optional.optional);
    assert!(!optional.enabled_by_default());
    assert_eq!(optional.default_port, 7000);
    Ok(())
}

#[test]
fn a_sixth_profile_alongside_the_five_loads_without_interference() -> TestResult {
    let scratch = Scratch::new("sixth-plus")?;
    scratch.write("z-obsidian-drift.json", &sixth_profile_json())?;
    for entry in fs::read_dir(repo_profiles_dir())? {
        let path = entry?.path();
        let Some(name) = path.file_name() else {
            continue;
        };
        fs::copy(&path, scratch.dir().join(name))?;
    }

    let registry = Registry::load_dir(scratch.dir())?;
    assert_eq!(registry.len(), EXPECTED_DEFAULTS.len() + 1);
    assert!(registry.get("obsidian-drift").is_some());
    assert!(registry.get("minecraft-java").is_some());
    Ok(())
}

// ---------------------------------------------------------------------------
// GAME-007: invalid profiles are rejected, with a reason
// ---------------------------------------------------------------------------

#[test]
fn invalid_profiles_are_rejected_with_a_reason() -> TestResult {
    // One field wrong at a time, so a failure names the rule that broke. Every case must be
    // refused; a case that loads is a validation rule that does not exist.
    let long_name = "x".repeat(200);
    let cases: Vec<(&str, String)> = vec![
        (
            "id-breaking-the-identifier-rule",
            minimal_json().replace("\"sample-game\"", "\"Sample_Game\""),
        ),
        ("id-empty", minimal_json().replace("\"sample-game\"", "\"\"")),
        (
            "version-zero",
            minimal_json().replace("\"version\": 1", "\"version\": 0"),
        ),
        (
            "version-negative",
            minimal_json().replace("\"version\": 1", "\"version\": -3"),
        ),
        (
            "display-name-empty",
            minimal_json().replace("\"Sample Game\"", "\"\""),
        ),
        (
            "display-name-too-long",
            minimal_json().replace("\"Sample Game\"", &format!("\"{long_name}\"")),
        ),
        (
            "no-mappings",
            r#"{"id":"empty-game","version":1,"display_name":"Empty","mappings":[]}"#.to_string(),
        ),
        (
            "duplicate-mapping-keys",
            r#"{"id":"dup-game","version":1,"display_name":"Dup","mappings":[
                 {"key":"game","protocol":"tcp","default_port":1,"optional":false,"evidence":"verified"},
                 {"key":"game","protocol":"udp","default_port":2,"optional":true,"evidence":"verified"}]}"#
                .to_string(),
        ),
        (
            "mapping-key-breaks-the-rule",
            minimal_json().replace("\"key\": \"game\"", "\"key\": \"Game Port\""),
        ),
    ];

    for (name, body) in &cases {
        let scratch = Scratch::new(name)?;
        scratch.write("bad.json", body)?;
        match Registry::load_dir(scratch.dir()) {
            Ok(loaded) => {
                return Err(format!(
                    "{name}: loaded {:?}, but every case here must be rejected",
                    loaded.profiles().iter().map(|p| &p.id).collect::<Vec<_>>()
                )
                .into());
            }
            Err(e) => {
                // GAME-007 requires "a clear error": a rejection with nothing to read is not one.
                let rendered = e.to_string();
                assert!(!rendered.is_empty(), "{name}: rejected without a reason");
            }
        }
    }
    Ok(())
}

#[test]
fn every_broken_rule_is_reported_not_just_the_first() -> TestResult {
    // A profile author should fix one file in one pass. Four problems, four messages.
    let scratch = Scratch::new("all-at-once")?;
    scratch.write(
        "bad.json",
        r#"{"id":"BAD_ID","version":0,"display_name":"","mappings":[
             {"key":"GAME","protocol":"tcp","default_port":1,"optional":false,"evidence":"verified"}]}"#,
    )?;

    match Registry::load_dir(scratch.dir()) {
        Err(RegistryError::Invalid { error, .. }) => {
            assert!(
                error.problems.len() >= 4,
                "expected id, version, display_name and key problems, got {:?}",
                error.problems
            );
            for p in &error.problems {
                assert!(
                    !p.field.is_empty() && !p.problem.is_empty(),
                    "each problem must name a field and say what is wrong"
                );
            }
            Ok(())
        }
        other => Err(format!("expected Invalid, got {other:?}").into()),
    }
}

#[test]
fn structural_faults_are_rejected_by_the_types() -> TestResult {
    // These cannot be produced by constructing a `Profile` in Rust, so a file is the only way
    // to test them. Each is a fault the deserializer must refuse, which is why they surface as
    // `Malformed` rather than `Invalid`.
    let cases: Vec<(&str, String)> = vec![
        (
            "unknown-protocol",
            minimal_json().replace("\"tcp\"", "\"sctp\""),
        ),
        ("port-above-u16", minimal_json().replace("1234", "70000")),
        ("port-negative", minimal_json().replace("1234", "-1")),
        (
            "missing-field",
            minimal_json().replace("\"optional\": false, ", ""),
        ),
        ("not-an-object", r#"["sample-game"]"#.to_string()),
        ("not-json", r#"{ this is not json"#.to_string()),
        (
            // INV-1: a profile has no field in which to name a local destination. If one is
            // added later, this test fails rather than the destination quietly becoming
            // reachable data.
            "local-destination-field",
            minimal_json().replace(
                r#""optional": false"#,
                r#""optional": false, "local_host": "192.168.1.10", "local_port": 25565"#,
            ),
        ),
        (
            // The same trap under a different name, because the first being rejected proves
            // nothing about the second.
            "target-field",
            minimal_json().replace(
                r#""optional": false"#,
                r#""optional": false, "target": "127.0.0.1:25565""#,
            ),
        ),
        (
            "evidence-not-in-the-set",
            minimal_json().replace("\"verified\"", "\"assumed\""),
        ),
        (
            "bool-where-a-port-belongs",
            minimal_json().replace("\"default_port\": 1234", "\"default_port\": true"),
        ),
    ];

    for (name, body) in &cases {
        let scratch = Scratch::new(name)?;
        scratch.write("bad.json", body)?;
        match Registry::load_dir(scratch.dir()) {
            Ok(_) => return Err(format!("{name}: loaded, but must be refused").into()),
            Err(RegistryError::Malformed { detail, .. }) => {
                assert!(!detail.is_empty(), "{name}: refused without a reason");
            }
            Err(other) => {
                return Err(format!("{name}: expected Malformed, got {other:?}").into());
            }
        }
    }
    Ok(())
}

#[test]
fn a_profile_file_larger_than_the_bound_is_refused_before_it_is_read() -> TestResult {
    let scratch = Scratch::new("too-large")?;
    // Valid JSON throughout, so size is the only available reason to refuse it.
    let filler = "x".repeat(2000);
    let body = format!(
        r#"{{"id":"huge","version":1,"display_name":"{filler}","mappings":[
             {{"key":"game","protocol":"tcp","default_port":1,"optional":false,"evidence":"verified"}}]}}"#
    );
    assert!(
        body.len() as u64 > gtt_profiles::MAX_PROFILE_BYTES,
        "the fixture must exceed the bound for this test to mean anything"
    );
    scratch.write("huge.json", &body)?;

    match Registry::load_dir(scratch.dir()) {
        Err(RegistryError::TooLarge { name, size, limit }) => {
            assert_eq!(name, "huge.json");
            assert_eq!(limit, gtt_profiles::MAX_PROFILE_BYTES);
            assert!(size > gtt_profiles::MAX_PROFILE_BYTES);
            Ok(())
        }
        other => Err(format!("expected TooLarge, got {other:?}").into()),
    }
}

#[test]
fn too_many_mappings_are_refused() -> TestResult {
    let mut mappings = String::new();
    for i in 0..(gtt_profiles::MAX_MAPPINGS + 5) {
        if i > 0 {
            mappings.push(',');
        }
        mappings.push_str(&format!(
            r#"{{"key":"m{i}","protocol":"tcp","default_port":{},"optional":true,"evidence":"verified"}}"#,
            i + 1
        ));
    }
    let scratch = Scratch::new("too-many")?;
    scratch.write(
        "wide.json",
        &format!(r#"{{"id":"wide","version":1,"display_name":"Wide","mappings":[{mappings}]}}"#),
    )?;

    match Registry::load_dir(scratch.dir()) {
        Err(RegistryError::Invalid { error, .. }) => {
            let mentions_limit = error.problems.iter().any(|p| p.problem.contains("at most"));
            assert!(
                mentions_limit,
                "the mapping-count limit should be reported, got: {error}"
            );
            Ok(())
        }
        other => Err(format!("expected Invalid, got {other:?}").into()),
    }
}

// ---------------------------------------------------------------------------
// Registry behaviour
// ---------------------------------------------------------------------------

#[test]
fn a_missing_directory_is_an_error_not_an_empty_registry() -> TestResult {
    // Otherwise a caller that ignores the result sees "no profiles available" and cannot tell
    // a build mistake from a bug in its own code.
    let missing = std::env::temp_dir().join(format!("gtt-profile-absent-{}", uuid::Uuid::new_v4()));
    match Registry::load_dir(&missing) {
        Err(RegistryError::NoProfilesDirectory { path, .. }) => {
            assert_eq!(path, missing);
            Ok(())
        }
        other => Err(format!("expected NoProfilesDirectory, got {other:?}").into()),
    }
}

#[test]
fn an_empty_directory_is_an_error() -> TestResult {
    let scratch = Scratch::new("empty")?;
    match Registry::load_dir(scratch.dir()) {
        Err(RegistryError::Empty { path }) => {
            assert_eq!(path, scratch.dir());
            Ok(())
        }
        other => Err(format!("expected Empty, got {other:?}").into()),
    }
}

#[test]
fn non_profile_files_are_ignored() -> TestResult {
    // A README or a licence in the directory is not an error, and is certainly not a profile.
    let scratch = Scratch::new("extra-files")?;
    scratch.write("profile.json", &minimal_json())?;
    fs::write(scratch.dir().join("README.md"), "notes")?;
    fs::write(scratch.dir().join("notes.txt"), "notes")?;

    let registry = Registry::load_dir(scratch.dir())?;
    assert_eq!(registry.len(), 1);
    Ok(())
}

#[test]
fn two_files_claiming_one_id_are_refused_and_both_are_named() -> TestResult {
    let scratch = Scratch::new("duplicate-id")?;
    scratch.write("a-first.json", &minimal_json())?;
    scratch.write(
        "b-second.json",
        &minimal_json().replace("\"version\": 1", "\"version\": 2"),
    )?;

    match Registry::load_dir(scratch.dir()) {
        Err(RegistryError::DuplicateId { id, first, second }) => {
            assert_eq!(id, "sample-game");
            assert_eq!(first, "a-first.json");
            assert_eq!(second, "b-second.json");
            Ok(())
        }
        other => Err(format!("expected DuplicateId, got {other:?}").into()),
    }
}

// ---------------------------------------------------------------------------
// The derivation that carries GAME-006 and GAME-008
// ---------------------------------------------------------------------------

#[test]
fn default_enablement_is_exactly_not_optional_and_verified() {
    // The load-bearing property. Stated over the whole cross-product with the reason next to
    // each row, so changing the expression has to change this test.
    let cases = [
        // (optional, evidence, expected, why)
        (
            false,
            Evidence::Verified,
            true,
            "the game works and the port is verified",
        ),
        (
            false,
            Evidence::Reported,
            false,
            "an unverified port is opt-in",
        ),
        (
            true,
            Evidence::Verified,
            false,
            "GAME-008: optional mappings are off by default",
        ),
        (true, Evidence::Reported, false, "off twice over"),
    ];

    for (optional, evidence, expected, why) in cases {
        let m = Mapping {
            key: "game".to_string(),
            protocol: Protocol::Tcp,
            default_port: 1,
            optional,
            evidence,
        };
        assert_eq!(
            m.enabled_by_default(),
            expected,
            "optional={optional} evidence={} should be {expected}: {why}",
            evidence.as_str()
        );
    }
}

#[test]
fn an_unverified_game_is_opt_in_rather_than_excluded() -> TestResult {
    // The distinction matters. GAME-006 forbids unverified *claims*; it does not make an
    // unverified game unplayable. The profile must load, be listable, and offer its mapping to
    // a host who asks -- otherwise the tool would make the research decision for them, which
    // is not what the requirement says.
    let registry = Registry::load_dir(&repo_profiles_dir())?;
    let Some(l4d1) = registry.get("l4d1") else {
        return Err("L4D1 is a shipped profile".into());
    };

    assert!(
        l4d1.default_enabled_keys().is_empty(),
        "nothing exposed by default"
    );
    let Some(game) = l4d1.mapping("game") else {
        return Err("the mapping is absent, which would be exclusion not opt-in".into());
    };
    assert_eq!(game.evidence, Evidence::Reported);
    assert!(!game.enabled_by_default());
    assert_eq!(
        game.default_port, 27015,
        "but the port is there for a host who asks"
    );
    Ok(())
}
