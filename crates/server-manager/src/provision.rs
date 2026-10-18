//! Which games may be provisioned at all (`SRV-003`).
//!
//! ## The list is empty today, and that is the feature working
//!
//! `SRV-003`: "Only games whose acquisition path has been verified can be provisioned", verified
//! by "The provisionable list equals the set of profiles with a passed acquisition experiment".
//!
//! No acquisition experiment has passed. `research.md` RQ-13 records E-15 as *partially answered
//! 2026-10-17 (desk research only)*, with the `server.jar` digest source marked `[X] not
//! confirmed` and the explicit note that "E-15 must verify the real check end to end before
//! `SRV-002` can be claimed". E-16 (Bedrock) records its official download page as **not
//! reachable** from the research environment.
//!
//! So the correct provisionable set is **empty**, and a caller that asks to provision anything
//! is refused. A default that allowed a game until someone remembered to remove it would be the
//! opposite of the requirement: `SRV-003` exists to stop an unverified path from being used by
//! omission rather than by decision.
//!
//! ## Why this is a set and not a lookup on the profile
//!
//! The tempting design is a `provisionable: bool` field on the profile, next to the evidence
//! label added in T-032. It is wrong, for a reason worth stating: **acquisition evidence and
//! port evidence are different facts about different questions.** `research.md` verifies
//! Minecraft Java's ports from the wiki while leaving the artifact's digest unverified. A
//! profile that said "I know the ports" could not also say "I know how to obtain the server",
//! and collapsing the two would let one answer stand in for the other.

use std::fmt;

use gtt_profiles::Profile;

/// One game whose acquisition path has been verified end to end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquisitionRecord {
    /// The profile id this record unblocks.
    pub profile_id: String,
    /// The experiment that verified it, e.g. `"E-15"`.
    pub experiment: String,
    /// Where the artifact comes from. Recorded so the value is auditable, not just asserted.
    pub source: String,
    /// How the artifact's integrity is checked, in the same terms `research.md` uses.
    pub verification: String,
}

impl fmt::Display for AcquisitionRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} via {} ({}), verified by {}",
            self.profile_id, self.source, self.experiment, self.verification
        )
    }
}

/// The set of games that may be provisioned.
#[derive(Debug, Clone, Default)]
pub struct ProvisionableSet {
    records: Vec<AcquisitionRecord>,
}

impl ProvisionableSet {
    /// The empty set.
    ///
    /// **The current, correct state.** See the module docs: no acquisition experiment has passed,
    /// so nothing is provisionable and `SRV-003` is satisfied by refusing everything.
    pub fn none() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    /// Build from a list of verified acquisitions.
    ///
    /// Exists so that when E-15 does pass end to end, adding a game is a one-line change here
    /// with a recorded source and verification method, rather than a code path invented later
    /// under time pressure.
    pub fn from_verified(records: Vec<AcquisitionRecord>) -> Self {
        Self { records }
    }

    /// Whether this profile may be provisioned.
    pub fn contains(&self, profile_id: &str) -> bool {
        self.records.iter().any(|r| r.profile_id == profile_id)
    }

    /// How many games are provisionable.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether nothing is provisionable.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// The profile ids that are provisionable, in recorded order.
    pub fn profile_ids(&self) -> impl Iterator<Item = &str> {
        self.records.iter().map(|r| r.profile_id.as_str())
    }

    /// The record for a profile, if it has one.
    pub fn record_for(&self, profile_id: &str) -> Option<&AcquisitionRecord> {
        self.records.iter().find(|r| r.profile_id == profile_id)
    }

    /// Check a profile against the set, naming the experiment that would be needed.
    ///
    /// Returns `Ok(())` only for a profile with a verified acquisition. The refusal message says
    /// what is missing, because "provisioning is disabled" with no reason is not something a
    /// host can act on.
    pub fn require(&self, profile: &Profile) -> Result<&AcquisitionRecord, ProvisionError> {
        match self.record_for(&profile.id) {
            Some(record) => Ok(record),
            None => Err(ProvisionError::NotProvisionable {
                profile_id: profile.id.clone(),
                reason: self.reason_for(&profile.id),
            }),
        }
    }

    /// Why a profile is not provisionable, in terms a host can act on.
    fn reason_for(&self, _profile_id: &str) -> String {
        if self.records.is_empty() {
            "no game's acquisition path has been verified end to end yet (SRV-003); \
             research.md RQ-13 records E-15 as partially answered and E-16's download page \
             as unreachable"
                .to_string()
        } else {
            format!(
                "this game's acquisition path has not been verified; {} other game(s) have \
                 been",
                self.records.len()
            )
        }
    }
}

/// Why provisioning was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvisionError {
    /// The profile has no verified acquisition record.
    NotProvisionable {
        /// The profile's id.
        profile_id: String,
        /// Why, in terms a host can act on.
        reason: String,
    },
}

impl fmt::Display for ProvisionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotProvisionable { profile_id, reason } => {
                write!(f, "{profile_id} cannot be provisioned: {reason}")
            }
        }
    }
}

impl std::error::Error for ProvisionError {}
