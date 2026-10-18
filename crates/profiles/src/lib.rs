//! Declarative game profiles: the model, its validation, and the directory loader.
//!
//! # Why the evidence label decides whether a mapping is exposed
//!
//! `GAME-006` and `SRV-003` share one rule: no unverified claims. `research.md` RQ-1 records
//! two facts about the five initial games that are **not** verified:
//!
//! * Terraria's UDP port is genuinely unresolved -- the source page says "TCP port 7777" in
//!   one sentence and "port 7777 on TCP **and UDP**" in another. Both are on the same page,
//!   and E-1 stands to settle it.
//! * Left 4 Dead (1) is `[R]` throughout: "Weakest evidence of the five. Needs a primary
//!   source or experiment."
//!
//! A profile file that simply listed those mappings would turn a `[R]` into something that
//! looks authoritative the moment a tunnel is created from it, which is precisely what the
//! discipline forbids. So `Evidence` is part of the model and feeds
//! [`Mapping::enabled_by_default`]:
//!
//! ```text
//! enabled_by_default = !optional && evidence == Verified
//! ```
//!
//! **Derived, not stored.** `GAME-008` ("optional mappings are disabled by default") and
//! `GAME-006` ("no unverified claims") both become structural -- there is no field to set
//! inconsistently, so no check is needed to catch the violation. This is the same reasoning
//! that made the T-041 persistence layer refuse to store a field it could derive. A checked
//! invariant and a structure that cannot express the violation are not equally strong, and
//! the second needs no test to keep working.
//!
//! The consequence is deliberate and visible: **the L4D1 profile loads and validates, but
//! exposes nothing by default.** That is not a defect. It is an unverified game being
//! opt-in, which is what "weakest evidence of the five" should mean in a tool that routes
//! traffic. Recorded as an ADR candidate in `research.md`; see `tasks.md` T-032.
//!
//! # Why `optional` also feeds the same expression
//!
//! `optional` means "the game works without it" -- query, RCON, SourceTV. Such a mapping is
//! not needed and so is not exposed, which is `GAME-008` verbatim. Deriving from both
//! conditions keeps a single source of truth; storing `enabled` alongside `optional` would
//! create two fields that must agree and a check that exists only because they might not.
//!
//! # Boundaries
//!
//! This crate is a leaf: it depends on `serde` and `serde_json` and nothing else, and
//! `architecture.md` records its outbound dependencies as "none" beyond that. It has no
//! network access, no filesystem writes, and no knowledge of any specific game -- adding a
//! sixth profile file must not require a code change (`GAME-007`), which
//! `tests/profiles.rs` checks by loading a profile for a game this source has never heard of.
//!
//! Every profile type denies unknown fields. That is how **INV-1** is enforced here: a
//! profile cannot declare a local destination because there is no field to declare it in,
//! and an unrecognised key is rejected rather than ignored.

mod model;
mod registry;
mod validate;

pub use model::{Evidence, Mapping, Profile, Protocol};
pub use registry::{Registry, RegistryError, MAX_PROFILE_BYTES, MAX_PROFILE_FILES};
pub use validate::{
    is_valid_identifier, ProfileError, ValidationError, MAX_DISPLAY_NAME_LEN, MAX_IDENTIFIER_LEN,
    MAX_MAPPINGS,
};

use std::path::Path;

/// Load every profile file in `dir`.
///
/// Convenience over [`Registry::load_dir`]; the two exist so a caller can express which it
/// meant.
pub fn load_dir(dir: &Path) -> Result<Registry, RegistryError> {
    Registry::load_dir(dir)
}
