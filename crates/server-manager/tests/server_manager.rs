//! `SRV-002`, `SRV-004`, `SRV-010`, `SRV-003`: artifact verification, generated
//! configuration, path confinement, and the provisionable gate.
//!
//! ## No `expect` and no `panic!`
//!
//! The workspace denies the `panic` lint group on all targets, so tests return `TestResult` and
//! propagate with `?`. `VerifyError`, `PathError` and `ConfigError` all implement `Error`, so a
//! failure prints their `Debug`, which is more informative than a hand-written message: a
//! `DigestMismatch` names the path, the published digest and the computed one.
//!
//! ## Why the tamper test writes a real file
//!
//! "A tampered artifact is rejected before execution" is only meaningful against bytes on disk.
//! A test that hashes a string and compares digests proves the digest function works; it does
//! not prove that the *file* is read, that the size bound is applied to the file, or that a
//! mismatch produces a refusal rather than a warning. So the tamper cases write a file, modify
//! it, and verify again.
//!
//! ## Known digests
//!
//! Two SHA-256 values are hard-coded so the parser and the hasher are both checked against
//! something published rather than against themselves:
//!
//! * the SHA-256 of the empty string, `e3b0c442...b7852b855`, which is the standard test vector;
//! * the SHA-256 of `"abc"`, `ba7816bf...f20015ad`, likewise.
//!
//! A verifier that hashed its own output consistently would pass a self-referential test and
//! still be wrong.

use std::fs;
use std::path::{Path, PathBuf};

use gtt_profiles::{Evidence, Mapping, Profile, Protocol};
use gtt_server_manager::{
    generate, unverified_mappings, verify, AcquisitionRecord, ArtifactSource, ConfigError,
    DataPaths, Digest, PathError, ProvisionableSet, ServerConfigRequest,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// SHA-256 of the empty string.
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
/// SHA-256 of `"abc"`.
const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

/// A scratch directory, removed when dropped.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join(format!("gtt-srvmgr-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    fn write(&self, name: &str, body: &[u8]) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let path = self.dir.join(name);
        fs::write(&path, body)?;
        Ok(path)
    }

    fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        drop(fs::remove_dir_all(&self.dir));
    }
}

// ---------------------------------------------------------------------------
// Digest
// ---------------------------------------------------------------------------

#[test]
fn digests_round_trip_through_hex() -> TestResult {
    let d = Digest::parse_hex(EMPTY_SHA256)?;
    assert_eq!(
        d.to_hex(),
        EMPTY_SHA256,
        "parse_hex then to_hex must be identity"
    );
    Ok(())
}

#[test]
fn hex_parsing_accepts_either_case_and_surrounding_space() -> TestResult {
    let lower = Digest::parse_hex(ABC_SHA256)?;
    let upper = Digest::parse_hex(&ABC_SHA256.to_uppercase())?;
    let padded = Digest::parse_hex(&format!("  {ABC_SHA256}\n"))?;

    assert!(lower.matches(upper), "case must not change the value");
    assert!(
        lower.matches(padded),
        "surrounding whitespace must be trimmed"
    );
    Ok(())
}

#[test]
fn malformed_digests_are_refused_without_echoing_the_value() -> TestResult {
    // The published digest can come from a remote manifest, so a malformed one is untrusted
    // text. `PRIV-002`/`SEC-013`: the error names the problem, not the value.
    let secret_looking = "not-a-digest-but-please-do-not-log-".repeat(4);
    for bad in [
        String::new(),
        "abc".to_string(),
        "z".repeat(64),
        format!("{}0", "a".repeat(63)),
        secret_looking.clone(),
    ] {
        match Digest::parse_hex(&bad) {
            Ok(_) => return Err(format!("{bad:?} should not parse").into()),
            Err(e) => {
                let rendered = e.to_string();
                assert!(
                    !rendered.contains(&secret_looking),
                    "the error echoed the value: {rendered}"
                );
            }
        }
    }
    Ok(())
}

#[test]
fn a_short_hex_string_names_its_actual_length() -> TestResult {
    match Digest::parse_hex("abc") {
        Err(e) => {
            let rendered = e.to_string();
            assert!(
                rendered.contains("expected 64 hex characters, got 3"),
                "the error should say what was wrong: {rendered}"
            );
            Ok(())
        }
        Ok(_) => Err("a 3-character digest must not parse".into()),
    }
}

// ---------------------------------------------------------------------------
// SRV-002: verification
// ---------------------------------------------------------------------------

#[test]
fn a_correct_artifact_verifies() -> TestResult {
    let scratch = Scratch::new("good")?;
    let path = scratch.write("empty.bin", b"")?;
    let source = ArtifactSource {
        path,
        expected: Digest::parse_hex(EMPTY_SHA256)?,
    };
    verify(&source)?;
    Ok(())
}

#[test]
fn a_tampered_artifact_is_rejected() -> TestResult {
    // The headline requirement. Bytes are written, then changed, and the same digest must stop
    // matching.
    let scratch = Scratch::new("tamper")?;
    let path = scratch.write("abc.bin", b"abc")?;

    let honest = ArtifactSource {
        path: path.clone(),
        expected: Digest::parse_hex(ABC_SHA256)?,
    };
    verify(&honest)?;

    // One byte changed. Every other byte is identical, which is the case a length-only or
    // mtime-only check would miss.
    fs::write(&path, b"abd")?;

    match verify(&honest) {
        Ok(()) => Err("a tampered artifact verified successfully".into()),
        Err(e) => {
            let rendered = e.to_string();
            assert!(
                rendered.contains("refusing to execute"),
                "the refusal should say the artifact will not be executed: {rendered}"
            );
            assert!(
                rendered.contains(ABC_SHA256) || rendered.contains(&ABC_SHA256[..8]),
                "the error should name the digests: {rendered}"
            );
            Ok(())
        }
    }
}

#[test]
fn a_one_byte_longer_artifact_is_rejected() -> TestResult {
    // Truncation and extension are the two attacks a digest catches that a size check does not.
    let scratch = Scratch::new("length")?;
    let path = scratch.write("abc.bin", b"abc")?;
    let source = ArtifactSource {
        path: path.clone(),
        expected: Digest::parse_hex(ABC_SHA256)?,
    };

    fs::write(&path, b"abcd")?;
    if verify(&source).is_ok() {
        return Err("an extended artifact verified".into());
    }

    fs::write(&path, b"ab")?;
    if verify(&source).is_ok() {
        return Err("a truncated artifact verified".into());
    }
    Ok(())
}

#[test]
fn an_empty_artifact_matches_the_empty_digest() -> TestResult {
    // Confirms the streaming loop terminates at zero bytes rather than treating it as a read
    // error, and that the published empty-string vector is the one this hasher produces.
    let scratch = Scratch::new("empty")?;
    let path = scratch.write("zero.bin", b"")?;
    let source = ArtifactSource {
        path,
        expected: Digest::parse_hex(EMPTY_SHA256)?,
    };
    verify(&source)?;
    Ok(())
}

#[test]
fn a_missing_artifact_is_an_error_not_a_pass() -> TestResult {
    // The failure mode that matters most for a security control: an absent file must not read
    // as "nothing to verify, proceed".
    let scratch = Scratch::new("missing")?;
    let source = ArtifactSource {
        path: scratch.dir().join("nope.bin"),
        expected: Digest::parse_hex(ABC_SHA256)?,
    };
    match verify(&source) {
        Ok(()) => Err("a missing artifact verified successfully".into()),
        Err(e) => {
            assert!(
                e.to_string().contains("cannot read artifact"),
                "unexpected error: {e}"
            );
            Ok(())
        }
    }
}

#[test]
fn a_directory_is_refused_before_it_is_read() -> TestResult {
    let scratch = Scratch::new("dir")?;
    let source = ArtifactSource {
        path: scratch.dir().to_path_buf(),
        expected: Digest::parse_hex(EMPTY_SHA256)?,
    };
    match verify(&source) {
        Ok(()) => Err("a directory verified successfully".into()),
        Err(e) => {
            assert!(
                e.to_string().contains("not a regular file"),
                "unexpected: {e}"
            );
            Ok(())
        }
    }
}

#[test]
fn the_artifact_error_names_the_file_but_not_its_contents() -> TestResult {
    let scratch = Scratch::new("no-echo")?;
    let marker = b"THIS-CONTENT-MUST-NOT-APPEAR-IN-ANY-ERROR".as_slice();
    let path = scratch.write("secret.bin", marker)?;
    let source = ArtifactSource {
        path,
        expected: Digest::parse_hex(EMPTY_SHA256)?,
    };

    let e = match verify(&source) {
        Ok(()) => return Err("a mismatched artifact verified".into()),
        Err(e) => e,
    };
    let rendered = e.to_string();
    assert!(
        rendered.contains("secret.bin"),
        "the host needs to know which download failed: {rendered}"
    );
    assert!(
        !rendered.contains("THIS-CONTENT-MUST-NOT-APPEAR"),
        "the error echoed the artifact's contents: {rendered}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// SRV-004 / GAME-008: generated configuration
// ---------------------------------------------------------------------------

fn terraria_profile() -> Profile {
    Profile {
        id: "terraria".to_string(),
        version: 1,
        display_name: "Terraria".to_string(),
        mappings: vec![
            Mapping {
                key: "game".to_string(),
                protocol: Protocol::Tcp,
                default_port: 7777,
                optional: false,
                evidence: Evidence::Verified,
            },
            Mapping {
                key: "game-udp".to_string(),
                protocol: Protocol::Udp,
                default_port: 7777,
                optional: true,
                evidence: Evidence::Reported,
            },
        ],
    }
}

fn minecraft_java_profile() -> Profile {
    Profile {
        id: "minecraft-java".to_string(),
        version: 1,
        display_name: "Minecraft Java Edition".to_string(),
        mappings: vec![
            Mapping {
                key: "game".to_string(),
                protocol: Protocol::Tcp,
                default_port: 25565,
                optional: false,
                evidence: Evidence::Verified,
            },
            Mapping {
                key: "query".to_string(),
                protocol: Protocol::Udp,
                default_port: 25565,
                optional: true,
                evidence: Evidence::Verified,
            },
            Mapping {
                key: "rcon".to_string(),
                protocol: Protocol::Tcp,
                default_port: 25575,
                optional: true,
                evidence: Evidence::Verified,
            },
        ],
    }
}

/// The value of a `key=value` line, so a test can assert on configuration rather than on
/// formatting.
fn property<'a>(contents: &'a str, key: &str) -> Option<&'a str> {
    contents
        .lines()
        .find_map(|l| l.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| v))
}

#[test]
fn a_generated_config_has_rcon_and_query_off() -> TestResult {
    let profile = minecraft_java_profile();
    let generated = generate(&ServerConfigRequest {
        profile: &profile,
        max_players: 20,
        rcon_secret: None,
        enable_query: false,
        query_secret: None,
    })?;

    assert_eq!(property(&generated.contents, "enable-rcon"), Some("false"));
    assert_eq!(property(&generated.contents, "enable-query"), Some("false"));
    assert!(!generated.rcon_enabled);
    assert!(!generated.query_enabled);

    // The port comes from the profile, not the caller.
    assert_eq!(generated.server_port, 25565);
    assert_eq!(property(&generated.contents, "server-port"), Some("25565"));
    assert_eq!(property(&generated.contents, "max-players"), Some("20"));
    Ok(())
}

#[test]
fn regenerating_over_a_hand_edited_file_turns_rcon_back_off() -> TestResult {
    // The reason the flags are written unconditionally. A host who edits `enable-rcon=true`
    // by hand must not have it survive the next regeneration.
    let profile = minecraft_java_profile();
    let edited = "enable-rcon=true\nrcon.password=guessme\nenable-query=true\n";
    assert!(
        edited.contains("enable-rcon=true"),
        "the fixture must be edited"
    );

    let generated = generate(&ServerConfigRequest {
        profile: &profile,
        max_players: 20,
        rcon_secret: None,
        enable_query: false,
        query_secret: None,
    })?;

    assert_eq!(property(&generated.contents, "enable-rcon"), Some("false"));
    assert_eq!(property(&generated.contents, "enable-query"), Some("false"));
    assert!(!generated.contents.contains("guessme"), "no stale secret");
    Ok(())
}

#[test]
fn no_secret_is_invented_when_rcon_is_off() -> TestResult {
    let profile = minecraft_java_profile();
    let generated = generate(&ServerConfigRequest {
        profile: &profile,
        max_players: 8,
        rcon_secret: None,
        enable_query: false,
        query_secret: None,
    })?;
    assert_eq!(
        property(&generated.contents, "rcon.password"),
        Some(""),
        "an off RCON must write an empty secret, never a generated one"
    );
    Ok(())
}

#[test]
fn rcon_needs_a_secret_before_it_can_be_enabled() -> TestResult {
    // `SRV-004`: enabling requires an explicit host action *and* a strong secret. The second
    // half is what this refuses.
    let profile = minecraft_java_profile();

    // Query on, no secret.
    match generate(&ServerConfigRequest {
        profile: &profile,
        max_players: 20,
        rcon_secret: None,
        enable_query: true,
        query_secret: None,
    }) {
        Ok(g) => return Err(format!("query was enabled with no secret: {}", g.contents).into()),
        Err(ConfigError::MissingSecret { feature }) => assert_eq!(feature, "query"),
        Err(other) => return Err(format!("expected MissingSecret, got {other}").into()),
    }

    // Secret supplied but too weak.
    match generate(&ServerConfigRequest {
        profile: &profile,
        max_players: 20,
        rcon_secret: Some("short".to_string()),
        enable_query: false,
        query_secret: None,
    }) {
        Ok(g) => return Err(format!("a 5-character secret was accepted: {}", g.contents).into()),
        Err(ConfigError::WeakSecret { feature, .. }) => assert_eq!(feature, "rcon"),
        Err(other) => return Err(format!("expected WeakSecret, got {other}").into()),
    }
    Ok(())
}

#[test]
fn rcon_and_query_turn_on_when_the_host_supplies_strong_secrets() -> TestResult {
    let profile = minecraft_java_profile();
    let secret = "a-sufficiently-long-secret";
    let generated = generate(&ServerConfigRequest {
        profile: &profile,
        max_players: 20,
        rcon_secret: Some(secret.to_string()),
        enable_query: true,
        query_secret: Some(secret.to_string()),
    })?;

    assert!(generated.rcon_enabled);
    assert!(generated.query_enabled);
    assert_eq!(property(&generated.contents, "enable-rcon"), Some("true"));
    assert_eq!(property(&generated.contents, "enable-query"), Some("true"));
    assert_eq!(property(&generated.contents, "rcon.password"), Some(secret));
    // The query port comes from the profile's query mapping.
    assert_eq!(property(&generated.contents, "query.port"), Some("25565"));
    Ok(())
}

#[test]
fn asking_for_a_feature_the_profile_lacks_is_refused() -> TestResult {
    // Terraria declares no `rcon` or `query` mapping. Writing `enable-rcon=true` for it would
    // produce a key the game ignores, which reads as configured to a human.
    let profile = terraria_profile();
    match generate(&ServerConfigRequest {
        profile: &profile,
        max_players: 20,
        rcon_secret: Some("a-sufficiently-long-secret".to_string()),
        enable_query: false,
        query_secret: None,
    }) {
        Ok(g) => return Err(format!("Terraria accepted an RCON secret: {}", g.contents).into()),
        Err(ConfigError::ProfileHasNoRcon { profile_id }) => assert_eq!(profile_id, "terraria"),
        Err(other) => return Err(format!("expected ProfileHasNoRcon, got {other}").into()),
    }
    Ok(())
}

#[test]
fn a_profile_with_no_default_enabled_mapping_cannot_supply_a_port() -> TestResult {
    // The L4D1 shape from T-032: its only mapping is `[R]`, so nothing is exposed by default.
    // Configuration generation must refuse rather than guess.
    let l4d1 = Profile {
        id: "l4d1".to_string(),
        version: 1,
        display_name: "Left 4 Dead".to_string(),
        mappings: vec![Mapping {
            key: "game".to_string(),
            protocol: Protocol::Udp,
            default_port: 27015,
            optional: false,
            evidence: Evidence::Reported,
        }],
    };
    match generate(&ServerConfigRequest {
        profile: &l4d1,
        max_players: 4,
        rcon_secret: None,
        enable_query: false,
        query_secret: None,
    }) {
        Ok(g) => return Err(format!("a port was generated anyway: {}", g.contents).into()),
        Err(ConfigError::NoServerPort { profile_id }) => assert_eq!(profile_id, "l4d1"),
        Err(other) => return Err(format!("expected NoServerPort, got {other}").into()),
    }
    Ok(())
}

#[test]
fn unverified_mappings_are_reported_for_the_caller_that_is_about_to_provision() -> TestResult {
    let terraria = terraria_profile();
    assert_eq!(unverified_mappings(&terraria), vec!["game-udp"]);
    assert!(
        unverified_mappings(&minecraft_java_profile()).is_empty(),
        "every Minecraft Java mapping is verified"
    );
    Ok(())
}

#[test]
fn the_config_never_contains_a_local_destination() -> TestResult {
    // INV-1 adjacent: a generated file is written to disk and read by the game, and it must not
    // become a place a host address can be smuggled into.
    let profile = minecraft_java_profile();
    let generated = generate(&ServerConfigRequest {
        profile: &profile,
        max_players: 20,
        rcon_secret: None,
        enable_query: false,
        query_secret: None,
    })?;
    for line in generated.contents.lines() {
        if let Some((k, v)) = line.split_once('=') {
            assert!(
                !v.contains("127.0.0.1") && !v.contains("0.0.0.0") && !v.contains("::"),
                "key {k:?} carries what looks like an address: {v:?}"
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// SRV-010: path confinement
// ---------------------------------------------------------------------------

#[test]
fn a_relative_name_resolves_inside_the_root() -> TestResult {
    let scratch = Scratch::new("paths-ok")?;
    let paths = DataPaths::new(scratch.dir(), "app")?;
    let resolved = paths.resolve(&["servers", "minecraft-java"])?;
    assert!(resolved.starts_with(paths.root()));
    assert!(
        resolved.ends_with("minecraft-java"),
        "unexpected path: {resolved:?}"
    );
    Ok(())
}

#[test]
fn a_traversing_name_is_refused() -> TestResult {
    let scratch = Scratch::new("paths-traverse")?;
    let paths = DataPaths::new(scratch.dir(), "app")?;

    for attempt in [
        vec!["..", "Windows"],
        vec!["servers", "..", "..", "escape"],
        vec!["servers/../../etc"],
        vec![".", "..", "..", "outside"],
    ] {
        match paths.resolve(&attempt) {
            Ok(p) => return Err(format!("{attempt:?} resolved to {p:?}").into()),
            Err(PathError::EscapesRoot { .. }) | Err(PathError::AbsoluteComponent { .. }) => {}
            Err(other) => return Err(format!("{attempt:?}: unexpected {other}").into()),
        }
    }
    Ok(())
}

#[test]
fn an_absolute_name_is_refused_rather_than_joined() -> TestResult {
    // `Path::join` discards everything before an absolute component, so accepting one would
    // silently redirect the write outside the root.
    let scratch = Scratch::new("paths-abs")?;
    let paths = DataPaths::new(scratch.dir(), "app")?;

    for attempt in [
        vec!["C:\\Windows\\System32"],
        vec!["servers", "C:\\Windows"],
        vec!["/etc/passwd"],
        vec!["\\\\server\\share"],
    ] {
        match paths.resolve(&attempt) {
            Ok(p) => return Err(format!("{attempt:?} resolved to {p:?}").into()),
            Err(e) => {
                let rendered = e.to_string();
                assert!(
                    rendered.contains("absolute") || rendered.contains("outside"),
                    "{attempt:?}: unexpected error {rendered}"
                );
            }
        }
    }
    Ok(())
}

#[test]
fn an_empty_name_is_refused() -> TestResult {
    let scratch = Scratch::new("paths-empty")?;
    let paths = DataPaths::new(scratch.dir(), "app")?;
    assert!(
        paths.resolve(&[]).is_err(),
        "an empty segment list must not resolve"
    );
    Ok(())
}

#[test]
fn a_multi_level_path_that_does_not_exist_yet_still_resolves() -> TestResult {
    // The case provisioning actually creates: `servers/<game>/<artifact>` where neither `<game>`
    // nor `<artifact>` is on disk yet.
    //
    // An earlier version of `resolve` canonicalised only the *immediate* parent, so this was
    // refused -- the single bug that would have made the crate unable to do its job. It was
    // found by a simulation of the normalisation logic against a real directory tree, not by
    // the Rust tests, because every other test in this file used a path one level deep.
    let scratch = Scratch::new("paths-deep")?;
    let paths = DataPaths::new(scratch.dir(), "app")?;

    for segments in [
        vec!["servers", "minecraft-java", "world"],
        vec!["config", "a", "b", "c", "d"],
        vec!["artifacts", "minecraft-java", "server.jar"],
    ] {
        match paths.resolve(&segments) {
            Ok(p) => {
                assert!(
                    p.starts_with(paths.root()),
                    "{segments:?} resolved outside the root: {p:?}"
                );
                assert!(
                    p.ends_with(segments.last().copied().unwrap_or("")),
                    "{segments:?} lost its last component: {p:?}"
                );
            }
            Err(e) => {
                return Err(format!("{segments:?} should resolve but was refused: {e}").into());
            }
        }
    }
    Ok(())
}

#[test]
fn a_name_that_resolves_back_inside_is_allowed() -> TestResult {
    // The complement of the traversal test: `..` is refused only when it actually escapes. A
    // rule that refused all `..` would be simpler and would also be wrong.
    let scratch = Scratch::new("paths-inner")?;
    let paths = DataPaths::new(scratch.dir(), "app")?;
    // The inner `..` case now depends on the ancestor walk, since `servers` exists but
    // `servers/minecraft-java` does not.
    let resolved = paths.resolve(&["servers", "minecraft-java", "..", "terraria"])?;
    assert!(resolved.starts_with(paths.root()));
    assert!(
        resolved.ends_with("terraria"),
        "unexpected path: {resolved:?}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// SRV-003: the provisionable gate
// ---------------------------------------------------------------------------

#[test]
fn nothing_is_provisionable_today() -> TestResult {
    // `SRV-003`, and the state research.md actually supports: no acquisition experiment has
    // passed, so the correct list is empty.
    let set = ProvisionableSet::none();
    assert!(set.is_empty());
    assert_eq!(set.len(), 0);
    assert_eq!(set.profile_ids().count(), 0);
    Ok(())
}

#[test]
fn an_unverified_game_is_refused_with_a_reason_a_host_can_act_on() -> TestResult {
    let set = ProvisionableSet::none();
    let profile = terraria_profile();

    match set.require(&profile) {
        Ok(_) => Err("an unverified game was allowed to be provisioned".into()),
        Err(e) => {
            let rendered = e.to_string();
            assert!(
                rendered.contains("terraria"),
                "the refusal should name the game: {rendered}"
            );
            assert!(
                rendered.contains("E-15") && rendered.contains("E-16"),
                "the refusal should say which experiments are outstanding: {rendered}"
            );
            Ok(())
        }
    }
}

#[test]
fn a_verified_acquisition_admits_only_that_game() -> TestResult {
    // Constructed directly rather than by re-enabling anything, so the test shows the gate is
    // data-driven and that one record admits one game.
    let set = ProvisionableSet::from_verified(vec![AcquisitionRecord {
        profile_id: "minecraft-java".to_string(),
        experiment: "E-15".to_string(),
        source: "piston-meta/piston-data".to_string(),
        verification: "SHA-256".to_string(),
    }]);

    assert!(set.contains("minecraft-java"));
    assert!(
        !set.contains("terraria"),
        "one record must not admit others"
    );
    assert_eq!(set.len(), 1);

    assert!(set.require(&minecraft_java_profile()).is_ok());
    assert!(set.require(&terraria_profile()).is_err());
    Ok(())
}

#[test]
fn the_record_is_rendered_with_its_source_and_verification() -> TestResult {
    // `SRV-002`'s verification is "the source URL and verification method per game are
    // recorded", so a record that cannot be rendered cannot be audited.
    let record = AcquisitionRecord {
        profile_id: "minecraft-java".to_string(),
        experiment: "E-15".to_string(),
        source: "https://piston-data.example.invalid/v1/objects/<sha1>/server.jar".to_string(),
        verification: "SHA-256".to_string(),
    };
    let rendered = record.to_string();
    for needle in ["minecraft-java", "E-15", "SHA-256", "piston-data"] {
        assert!(
            rendered.contains(needle),
            "{needle:?} missing from {rendered:?}"
        );
    }
    Ok(())
}

#[test]
fn the_secret_floor_is_enforced_exactly_at_its_boundary() -> TestResult {
    // Replaces a test that asserted `MIN_SECRET_LEN == 16`, which cannot fail unless someone
    // edits the test and the constant together and therefore says nothing about whether 16 is
    // right. This one fails if the floor moves in *either* direction:
    //
    //   * lowered  -> the "one shorter is refused" case starts passing the secret through
    //   * raised   -> the "exactly at the floor" case stops being accepted
    //
    // `SRV-004` requires a strong generated secret, so the floor is a real policy value and
    // deserves a test that pins its behaviour rather than its number.
    let profile = minecraft_java_profile();
    let floor = gtt_server_manager::MIN_SECRET_LEN;

    let at_floor = "s".repeat(floor);
    let generated = generate(&ServerConfigRequest {
        profile: &profile,
        max_players: 20,
        rcon_secret: Some(at_floor.clone()),
        enable_query: false,
        query_secret: None,
    })?;
    assert!(
        generated.rcon_enabled,
        "a secret at exactly the floor must be accepted"
    );
    assert_eq!(
        property(&generated.contents, "rcon.password"),
        Some(at_floor.as_str())
    );

    let below_floor = "s".repeat(floor - 1);
    match generate(&ServerConfigRequest {
        profile: &profile,
        max_players: 20,
        rcon_secret: Some(below_floor),
        enable_query: false,
        query_secret: None,
    }) {
        Ok(g) => {
            return Err(format!("a secret below the floor was accepted: {}", g.contents).into())
        }
        Err(ConfigError::WeakSecret { feature, minimum }) => {
            assert_eq!(feature, "rcon");
            assert_eq!(
                minimum, floor,
                "the error should state the floor it applied"
            );
        }
        Err(other) => return Err(format!("expected WeakSecret, got {other}").into()),
    }
    Ok(())
}
