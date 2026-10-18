//! Validation at load (`GAME-007`).
//!
//! ## What is checked here and what is not
//!
//! Split on purpose:
//!
//! * **Structural** facts are enforced by the *types*. `Protocol` is an enum, so `"sctp"` is
//!   rejected by the deserializer. `default_port` is a `u16`, so `70000` never becomes a
//!   mapping. `deny_unknown_fields` means a profile cannot carry a local address, which is
//!   how INV-1 holds without a check.
//! * **Cross-field** facts are checked here, because no single field can express them:
//!   duplicate mapping keys, an empty mapping list, a key that breaks the wire rule, a version
//!   that is not positive.
//!
//! Nothing here re-checks what the types already guarantee. A check that cannot fail is a
//! check that will eventually fail to be maintained.
//!
//! ## Errors name the field and say what was wrong
//!
//! `GAME-007` requires an invalid profile to be rejected "with a clear error". A load failure
//! is the one error a profile author will ever see in normal operation, so it carries the
//! field path and the rule that was broken. It never carries the file's contents: a profile
//! is repository data rather than user input, but the rule that no loader echoes untrusted
//! text into an error string is worth keeping for the day a profile arrives from elsewhere.

use std::fmt;

use crate::model::{Mapping, Profile};

/// Longest a profile's `id` or a mapping's `key` may be.
///
/// `contracts.md` sets `mapping_key` to `^[a-z][a-z0-9-]{0,31}$`, which caps the identifier
/// at 32 characters. The same rule is applied to `id` so a profile identifier can be used
/// wherever a mapping key can without translation.
pub const MAX_IDENTIFIER_LEN: usize = 32;

/// Longest `display_name`, to bound what the UI has to render.
pub const MAX_DISPLAY_NAME_LEN: usize = 80;

/// Most mappings one profile may declare.
///
/// A profile describes one game's ports. A source engine game has four or five; thirty-two
/// is generous while still bounding the work an untrusted file can ask for (AGENTS.md
/// section 4: no unbounded allocation on file input).
pub const MAX_MAPPINGS: usize = 32;

/// Why a profile was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// Dotted path to the offending value, e.g. `mappings[2].key`.
    pub field: String,
    /// What the rule was, in the author's terms.
    pub problem: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.problem)
    }
}

/// Every reason a profile was rejected, not just the first.
///
/// Reported together so a profile author fixes one file once instead of one error per
/// attempt. Cheap to collect and much kinder than the alternative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileError {
    /// The profile's `id` as written, or `"<unknown>"` if the file was too malformed to say.
    pub profile_id: String,
    /// Every rule that was broken.
    pub problems: Vec<ValidationError>,
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "profile {:?} is invalid", self.profile_id)?;
        for p in &self.problems {
            write!(f, "\n  {p}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ProfileError {}

/// Whether `s` matches the identifier rule from `contracts.md`.
///
/// Hand-rolled rather than pulled from a regex crate: it is one alternative of one character
/// class, and `AGENTS.md` section 5.2 says not to add a dependency for a few lines of code.
pub fn is_valid_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    if s.len() > MAX_IDENTIFIER_LEN {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Check one profile, collecting every problem.
///
/// Returns `Ok(())` rather than a `Profile` because validation does not transform: the
/// profile is either usable or it is not.
pub fn validate(profile: &Profile) -> Result<(), ProfileError> {
    let mut problems = Vec::new();

    if !is_valid_identifier(&profile.id) {
        problems.push(ValidationError {
            field: "id".to_string(),
            problem: format!(
                "must match ^[a-z][a-z0-9-]{{0,{MAX_IDENTIFIER_LEN}}}$ (the same rule \
                 contracts.md sets for mapping_key), got {:?}",
                truncate_for_message(&profile.id)
            ),
        });
    }

    if profile.version < 1 {
        problems.push(ValidationError {
            field: "version".to_string(),
            problem: format!(
                "must be a positive content revision, got {}",
                profile.version
            ),
        });
    }

    if profile.display_name.is_empty() {
        problems.push(ValidationError {
            field: "display_name".to_string(),
            problem: "must not be empty; the UI shows it to the host".to_string(),
        });
    } else if profile.display_name.chars().count() > MAX_DISPLAY_NAME_LEN {
        problems.push(ValidationError {
            field: "display_name".to_string(),
            problem: format!("must be at most {MAX_DISPLAY_NAME_LEN} characters"),
        });
    }

    if profile.mappings.is_empty() {
        problems.push(ValidationError {
            field: "mappings".to_string(),
            problem: "must declare at least one mapping; a game needs a port".to_string(),
        });
    } else if profile.mappings.len() > MAX_MAPPINGS {
        problems.push(ValidationError {
            field: "mappings".to_string(),
            problem: format!(
                "declares {} mappings, at most {MAX_MAPPINGS} are allowed",
                profile.mappings.len()
            ),
        });
    }

    for (i, m) in profile.mappings.iter().enumerate() {
        check_mapping(m, i, &profile.mappings, &mut problems);
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(ProfileError {
            profile_id: profile.id.clone(),
            problems,
        })
    }
}

/// Check one mapping against the identifier rule, the uniqueness rule, and the GAME-008 /
/// GAME-006 consistency rule.
///
/// The last of those is worth stating: a mapping marked `optional` **and** carrying
/// `evidence: "verified"` is legitimate (a verified optional port), and so is a
/// non-optional reported one. What is *not* legitimate is a profile author writing a note to
/// themselves that contradicts the label, so the check is that a mapping which exposes
/// nothing is either optional or unverified -- never both for no stated reason. That keeps
/// the two flags meaningful without inventing a third.
fn check_mapping(m: &Mapping, index: usize, all: &[Mapping], problems: &mut Vec<ValidationError>) {
    let at = |suffix: &str| format!("mappings[{index}].{suffix}");

    if !is_valid_identifier(&m.key) {
        problems.push(ValidationError {
            field: at("key"),
            problem: format!(
                "must match ^[a-z][a-z0-9-]{{0,{MAX_IDENTIFIER_LEN}}}$, got {:?}",
                truncate_for_message(&m.key)
            ),
        });
    }

    // Uniqueness is checked against every *other* mapping rather than against a set, so the
    // message can name the position of the earlier duplicate.
    for (j, other) in all.iter().enumerate() {
        if j != index && other.key == m.key {
            problems.push(ValidationError {
                field: at("key"),
                problem: format!("duplicates mappings[{j}].key; keys must be unique per profile"),
            });
        }
    }

    // `default_port` needs no range check: it is a `u16`, so a file carrying 70000 fails in
    // the deserializer and no `Mapping` with that port is ever constructed.

    if m.evidence == crate::model::Evidence::Verified && m.optional && m.enabled_by_default() {
        // Unreachable by construction: `enabled_by_default` is `!optional && verified`. Kept
        // as a stated invariant rather than an `unreachable!()`, which is in the denied
        // `panic` lint group, and so that the two definitions cannot drift apart unnoticed.
        problems.push(ValidationError {
            field: at("optional"),
            problem: "marked optional but would be enabled by default".to_string(),
        });
    }
}

/// Shorten text quoted back in an error, so a malformed file cannot produce an unbounded
/// error message.
fn truncate_for_message(s: &str) -> String {
    const LIMIT: usize = 40;
    let mut out: String = s.chars().take(LIMIT).collect();
    if s.chars().count() > LIMIT {
        out.push('…');
    }
    out
}
