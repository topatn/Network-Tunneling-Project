//! Generated server configuration, with RCON and query structurally off (`SRV-004`, `GAME-008`).
//!
//! ## The rule, and why it is a type rather than a check
//!
//! `SRV-004`: "A freshly provisioned server has query and RCON disabled; where the game
//! supports them, enabling one requires an explicit host action and a strong generated secret."
//! `GAME-008` says the same about the tunnel side.
//!
//! The obvious implementation writes `enable-rcon=false` and hopes nobody edits it. That fails
//! in two ways: a later code path adds `enable-rcon=true` and nothing notices, or a hand-edited
//! file is regenerated with the flag still on. So the generated text is produced from a request
//! type that **cannot represent** RCON or query being on without a caller having supplied a
//! secret, and [`ConfigError`] refuses the combination rather than emitting `true` and
//! trusting the caller.
//!
//! ## Ports come from the profile, not from the request
//!
//! The request does not carry ports. It names a profile, and the server port for each of the
//! profile's **default-enabled** mappings is read from the profile. That keeps a caller from
//! generating a configuration whose ports disagree with the tunnel the host will expose, which
//! is the same class of disagreement `INV-2` exists to prevent on the tunnel side.
//!
//! ## Never emits a secret by default
//!
//! `SEC-016` and `SRV-004` together: a generated `rcon.password` must be strong and must not be
//! written unless RCON is explicitly enabled. [`ServerConfigRequest`] has no field that causes
//! one to be invented at generation time; the caller supplies a secret it generated, or asks for
//! none and gets none.

use std::fmt;

use gtt_profiles::{Evidence, Profile};

/// Longest a generated `server.properties` may be.
///
/// Every line this module writes is short and the key set is fixed, so a real file is under a
/// kilobyte. The bound exists so a profile with an unreasonable number of mappings cannot turn
/// generation into an unbounded write (`AGENTS.md` section 4).
pub const MAX_CONFIG_BYTES: usize = 16 * 1024;

/// What to generate.
#[derive(Debug, Clone)]
pub struct ServerConfigRequest<'a> {
    /// The profile the server is for. Supplies the ports.
    pub profile: &'a Profile,
    /// How many players the server accepts. `server.properties` has no such key, so this is
    /// carried for the caller that launches the process; it does not appear in the file.
    pub max_players: u16,
    /// A secret for RCON, supplied by the caller. `None` leaves RCON off.
    pub rcon_secret: Option<String>,
    /// Whether to enable the query protocol. Only meaningful if the profile declares one.
    pub enable_query: bool,
    /// A secret for query, supplied by the caller. Required if `enable_query` is set.
    pub query_secret: Option<String>,
}

/// A generated `server.properties`, plus what it was generated from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedConfig {
    /// The file contents.
    pub contents: String,
    /// The server port written, from the profile's default-enabled TCP mapping.
    pub server_port: u16,
    /// Whether RCON was enabled. Reflects the request; the file agrees.
    pub rcon_enabled: bool,
    /// Whether query was enabled.
    pub query_enabled: bool,
}

/// Why a configuration could not be generated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// RCON was requested on, or a secret was supplied, but the profile declares no RCON
    /// mapping. Generating `enable-rcon` for a game that has no such setting would write a key
    /// the game ignores, which reads as "configured" to a human and does nothing.
    ProfileHasNoRcon {
        /// The profile's id.
        profile_id: String,
    },
    /// Query was requested on, but the profile declares no query mapping.
    ProfileHasNoQuery {
        /// The profile's id.
        profile_id: String,
    },
    /// RCON or query was enabled without a secret. `SRV-004` requires an explicit host action
    /// **and** a strong secret; this is the refusal that makes the second half structural.
    MissingSecret {
        /// Which feature wanted one: `"rcon"` or `"query"`.
        feature: &'static str,
    },
    /// A supplied secret was too short to be worth writing.
    WeakSecret {
        /// Which feature's secret: `"rcon"` or `"query"`.
        feature: &'static str,
        /// The minimum length required.
        minimum: usize,
    },
    /// The profile has no default-enabled mapping to take a server port from.
    NoServerPort {
        /// The profile's id.
        profile_id: String,
    },
    /// The profile's default-enabled port is not one this generator is willing to write.
    UnsupportedPort {
        /// The profile's id.
        profile_id: String,
        /// The port the profile asked for.
        port: u16,
    },
    /// The generated file exceeded [`MAX_CONFIG_BYTES`].
    TooLarge {
        /// Its length.
        len: usize,
        /// The bound.
        limit: usize,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProfileHasNoRcon { profile_id } => {
                write!(f, "profile {profile_id:?} declares no rcon mapping")
            }
            Self::ProfileHasNoQuery { profile_id } => {
                write!(f, "profile {profile_id:?} declares no query mapping")
            }
            Self::MissingSecret { feature } => {
                write!(f, "{feature} cannot be enabled without a secret")
            }
            Self::WeakSecret { feature, minimum } => {
                write!(
                    f,
                    "the {feature} secret must be at least {minimum} characters"
                )
            }
            Self::NoServerPort { profile_id } => write!(
                f,
                "profile {profile_id:?} has no default-enabled mapping to take a server port from"
            ),
            Self::UnsupportedPort { profile_id, port } => {
                write!(
                    f,
                    "profile {profile_id:?} asks for port {port}, which is not writable"
                )
            }
            Self::TooLarge { len, limit } => {
                write!(
                    f,
                    "the generated file is {len} bytes, over the {limit}-byte bound"
                )
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// Shortest secret worth writing into a file.
pub const MIN_SECRET_LEN: usize = 16;

/// Generate a `server.properties`.
///
/// # Errors
///
/// Refuses any request that would enable RCON or query without a strong secret, that asks for a
/// feature the profile does not declare, or that the profile cannot supply a writable server
/// port for. It never returns a configuration with `enable-rcon=true` or `enable-query=true`
/// unless a secret of at least [`MIN_SECRET_LEN`] characters was supplied.
pub fn generate(request: &ServerConfigRequest<'_>) -> Result<GeneratedConfig, ConfigError> {
    let profile = request.profile;

    let rcon_mapping = profile.mapping("rcon");
    let query_mapping = profile.mapping("query");

    // Decide the flags first, refusing rather than emitting `true` on incomplete input.
    let rcon_enabled = request.rcon_secret.is_some();
    let query_enabled = request.enable_query;

    if rcon_enabled && rcon_mapping.is_none() {
        return Err(ConfigError::ProfileHasNoRcon {
            profile_id: profile.id.clone(),
        });
    }
    if query_enabled && query_mapping.is_none() {
        return Err(ConfigError::ProfileHasNoQuery {
            profile_id: profile.id.clone(),
        });
    }

    let rcon_secret = check_secret(request.rcon_secret.as_deref(), "rcon", rcon_enabled)?;
    let query_secret = check_secret(request.query_secret.as_deref(), "query", query_enabled)?;

    // The server port comes from the profile's first default-enabled mapping, which is the same
    // mapping the tunnel would expose by default.
    let server_port = profile
        .mappings
        .iter()
        .find(|m| m.enabled_by_default())
        .map(|m| m.default_port)
        .ok_or_else(|| ConfigError::NoServerPort {
            profile_id: profile.id.clone(),
        })?;

    if server_port == 0 || matches!(rcon_mapping, Some(m) if m.default_port == 0) {
        return Err(ConfigError::UnsupportedPort {
            profile_id: profile.id.clone(),
            port: server_port,
        });
    }

    let mut out = String::with_capacity(512);
    out.push_str("# Generated by the host application. Edits are overwritten on regeneration.\n");
    out.push_str("#\n");
    out.push_str("# RCON and query are OFF unless this file says otherwise, and this file says\n");
    out.push_str("# otherwise only when the host explicitly enabled them (SRV-004, GAME-008).\n");
    push_kv(&mut out, "server-port", &server_port.to_string());
    push_kv(&mut out, "max-players", &request.max_players.to_string());
    push_kv(&mut out, "online-mode", "false");

    // The two flags whose default matters most. Written unconditionally so that regenerating a
    // file that someone edited by hand turns them back off.
    push_kv(
        &mut out,
        "enable-rcon",
        if rcon_enabled { "true" } else { "false" },
    );
    if rcon_enabled {
        push_kv(
            &mut out,
            "rcon.password",
            rcon_secret.as_deref().unwrap_or_default(),
        );
    } else {
        // Writing an empty key is what Minecraft's own default file does, and it is clearer than
        // omitting the line when a host reads the file.
        push_kv(&mut out, "rcon.password", "");
    }

    push_kv(
        &mut out,
        "enable-query",
        if query_enabled { "true" } else { "false" },
    );
    if query_enabled {
        push_kv(
            &mut out,
            "query.port",
            &query_mapping.map_or_else(String::new, |m| m.default_port.to_string()),
        );
        push_kv(
            &mut out,
            "query.secret",
            query_secret.as_deref().unwrap_or_default(),
        );
    }

    if out.len() > MAX_CONFIG_BYTES {
        return Err(ConfigError::TooLarge {
            len: out.len(),
            limit: MAX_CONFIG_BYTES,
        });
    }

    Ok(GeneratedConfig {
        contents: out,
        server_port,
        rcon_enabled,
        query_enabled,
    })
}

/// Validate a supplied secret, returning it unchanged when acceptable.
///
/// `required` is false when the feature is off, in which case an absent secret is correct and a
/// present one is still length-checked -- a caller that supplied a weak secret has made a
/// mistake worth reporting even if the feature happens to be off.
fn check_secret(
    secret: Option<&str>,
    feature: &'static str,
    required: bool,
) -> Result<Option<String>, ConfigError> {
    match secret {
        None if required => Err(ConfigError::MissingSecret { feature }),
        None => Ok(None),
        Some(s) if s.chars().count() < MIN_SECRET_LEN => Err(ConfigError::WeakSecret {
            feature,
            minimum: MIN_SECRET_LEN,
        }),
        Some(s) => Ok(Some(s.to_string())),
    }
}

fn push_kv(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push('=');
    out.push_str(value);
    out.push('\n');
}

/// Whether every mapping a profile declares is verified by the project's research.
///
/// Not used to gate generation -- a profile with a reported mapping still generates a valid
/// file, and the profile loader has already decided what to expose by default. Exposed because
/// a caller about to *provision* wants to state the caveat rather than discover it later.
pub fn unverified_mappings(profile: &Profile) -> Vec<&str> {
    profile
        .mappings
        .iter()
        .filter(|m| m.evidence == Evidence::Reported)
        .map(|m| m.key.as_str())
        .collect()
}
