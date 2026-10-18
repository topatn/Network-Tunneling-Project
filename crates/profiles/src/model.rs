//! The profile model: the data a profile file carries, and nothing else.
//!
//! Every type here denies unknown fields, which is what makes **INV-1** structural rather
//! than checked. A profile file has no field in which a local address or port could be
//! written, so it cannot become a source of one, and a typo like `local_host` is a load error
//! instead of a silently ignored line.
//!
//! `Protocol` and the port width are both load-bearing. `Protocol` is an enum, so `"sctp"` is
//! rejected by the deserializer rather than by a check someone could forget. The port is a
//! `u16`, so `70000` is rejected the same way -- there is no out-of-range value in which a
//! mapping exists at all.

use serde::Deserialize;

/// Wire protocol for a mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    /// TCP, including RCON.
    Tcp,
    /// UDP.
    Udp,
}

impl Protocol {
    /// The wire value, matching `contracts.md`'s `protocol` field.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
        }
    }
}

/// How well the project's research supports a mapping's default port.
///
/// This is not a comment on the game. It is a record of what `research.md` RQ-1 says, carried
/// into the data so that the loader can act on it instead of a human having to remember.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Evidence {
    /// `[V]` in `research.md`: cited to a primary source, or corroborated by two independent
    /// ones.
    Verified,
    /// `[R]`: reported but uncited, or contradicted by its own source. Exposed only when the
    /// host explicitly asks.
    Reported,
}

impl Evidence {
    /// The wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Reported => "reported",
        }
    }
}

/// One port a game needs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mapping {
    /// Profile-defined name for this port, e.g. `game`, `query`, `rcon`.
    ///
    /// Must match `^[a-z][a-z0-9-]{0,31}$`, the rule `contracts.md` sets for a tunnel's
    /// `mapping_key`, so a key valid in a profile is valid on the wire without translation.
    pub key: String,

    /// Protocol this port uses.
    pub protocol: Protocol,

    /// The port the game listens on locally, used as the host's suggested value.
    ///
    /// A **port**, never an address. `INV-1` forbids a local destination from the control
    /// plane, and the host chooses the address; see [`Mapping::enabled_by_default`].
    pub default_port: u16,

    /// Whether the game works without this mapping.
    ///
    /// True for query, RCON and SourceTV. Drives `GAME-008`: see
    /// [`Mapping::enabled_by_default`].
    pub optional: bool,

    /// Whether `research.md` verifies this port.
    pub evidence: Evidence,
}

impl Mapping {
    /// Whether a tunnel created from this profile exposes this mapping without being asked.
    ///
    /// `!optional && evidence == Verified`, and **deliberately not a field.**
    ///
    /// Both inputs are things a profile author must think about anyway, so deriving from them
    /// means `GAME-006` and `GAME-008` cannot be violated by editing one field. Storing a
    /// third `enabled` flag would create three fields that must agree and a check that
    /// exists only because they might not.
    ///
    /// A `reported` mapping stays loadable and selectable; it just is not exposed by default.
    /// That is the difference between an unverified game being **opt-in** and being
    /// **excluded**.
    pub fn enabled_by_default(&self) -> bool {
        !self.optional && self.evidence == Evidence::Verified
    }
}

/// One game, at one version.
///
/// A file describes exactly one `(id, version)`. `version` is a content revision, not a
/// schema version: `data-model.md` records that `tunnels.profile_version` is stored per
/// tunnel and that changing it affects live tunnels, so a profile whose mappings changed gets
/// a new number and tunnels keep the behaviour they were created with.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// Profile identifier, stored in `tunnels.game_profile_id`.
    ///
    /// Same `^[a-z][a-z0-9-]{0,31}$` rule as a mapping key.
    pub id: String,

    /// Content revision. Positive, and bumped when the mappings change.
    pub version: i32,

    /// Human-readable name for the UI. Not an identifier; may contain any text.
    pub display_name: String,

    /// Every port this game needs. Never empty -- a profile that exposes nothing is not a
    /// profile, and L4D1's reported-only mapping still qualifies.
    pub mappings: Vec<Mapping>,
}

impl Profile {
    /// Look up a mapping by key.
    ///
    /// `None` for an unknown key rather than an error: "this profile does not declare that
    /// key" is the normal negative case, and matching `ProfileSpec::mapping` in the
    /// control plane keeps the two from disagreeing about what absence means.
    pub fn mapping(&self, key: &str) -> Option<&Mapping> {
        self.mappings.iter().find(|m| m.key == key)
    }

    /// The keys exposed by default, in declaration order.
    pub fn default_enabled_keys(&self) -> Vec<&str> {
        self.mappings
            .iter()
            .filter(|m| m.enabled_by_default())
            .map(|m| m.key.as_str())
            .collect()
    }
}
