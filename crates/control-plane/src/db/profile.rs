//! The slice of the game-profile registry that persistence needs in order to enforce
//! INV-2.
//!
//! ## Why this is a value and not a dependency on `gtt-profiles`
//!
//! INV-2 requires that a mapping's key and protocol match *the tunnel's profile
//! version*. Answering that needs the profile, but **loading profiles is T-032 and has
//! not happened**: `profiles/` is still empty and `gtt-profiles` is a skeleton. Rather
//! than implement T-032's loader inside T-034, or invent a `control-plane ->
//! gtt-profiles` dependency edge before either crate has any content, persistence
//! accepts the profile as a plain value supplied by its caller.
//!
//! The HTTP layer (T-040) will obtain this from the real registry and pass it in. The
//! alternative — persistence reaching for profiles itself — would make the persistence
//! layer unable to be tested without a profile registry, and would couple two crates
//! whose contents are both still undefined.

/// One mapping a profile version declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileMapping {
    /// Profile-defined key, e.g. a game port name.
    pub mapping_key: String,
    /// Protocol the profile requires for this key.
    pub protocol: crate::db::entities::Protocol,
    /// Whether the mapping is enabled by default.
    ///
    /// Recorded because `GAME-008` requires optional mappings to default to off. That
    /// rule is enforced where the profile is defined (T-032); persistence stores the
    /// resulting `enabled` flag and does not re-derive it.
    pub enabled_by_default: bool,
}

/// The mappings a single `(profile_id, version)` declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSpec {
    /// Profile identifier, matching `tunnels.game_profile_id`.
    pub profile_id: String,
    /// Profile version, matching `tunnels.profile_version`.
    pub version: i32,
    /// Every mapping this version declares.
    pub mappings: Vec<ProfileMapping>,
}

impl ProfileSpec {
    /// Look up the mapping a key names.
    ///
    /// Returns `None` for an unknown key rather than an error: "this profile does not
    /// declare that key" is the normal negative case INV-2 has to distinguish from a
    /// protocol mismatch, and making the caller handle both from one signature keeps the
    /// two rejection reasons from being confused.
    pub fn mapping(&self, mapping_key: &str) -> Option<&ProfileMapping> {
        self.mappings.iter().find(|m| m.mapping_key == mapping_key)
    }

    /// True when this spec is the one a tunnel records.
    pub fn matches(&self, profile_id: &str, profile_version: i32) -> bool {
        self.profile_id == profile_id && self.version == profile_version
    }

    /// Build a [`ProfileSpec`] from a loaded [`gtt_profiles::Profile`] (T-042).
    pub fn from_loaded(profile: &gtt_profiles::Profile) -> Self {
        Self {
            profile_id: profile.id.clone(),
            version: profile.version,
            mappings: profile
                .mappings
                .iter()
                .map(|m| ProfileMapping {
                    mapping_key: m.key.clone(),
                    protocol: match m.protocol {
                        gtt_profiles::Protocol::Tcp => crate::db::entities::Protocol::Tcp,
                        gtt_profiles::Protocol::Udp => crate::db::entities::Protocol::Udp,
                    },
                    enabled_by_default: m.enabled_by_default(),
                })
                .collect(),
        }
    }
}

impl From<&gtt_profiles::Profile> for ProfileSpec {
    fn from(profile: &gtt_profiles::Profile) -> Self {
        Self::from_loaded(profile)
    }
}
