//! Host-local game server artifact verification, configuration, and data paths.
//!
//! # Boundary
//!
//! Everything here runs **inside the desktop app's process tree**, on the host's own machine.
//! `architecture.md` is explicit: the server manager never talks to the relay or the control
//! plane, never receives a destination from the network, and never installs a service or asks
//! for elevation (`SRV-010`). There is no code path from a network message into this crate, and
//! [`paths`] is the piece that would notice if one appeared.
//!
//! # What is implemented, and what is deliberately not
//!
//! `T-035` asks for artifact *acquisition* from a channel recorded by E-15/E-16. Those
//! experiments are **not complete** — `research.md` RQ-13 records E-15's status as "partially
//! answered 2026-10-17 (desk research only)" and states, in `[X]` terms:
//!
//! > Not confirmed: that the manifest was actually fetched and that a manifest-sourced SHA-1
//! > verifies for the `server.jar` object specifically (the manifest hashes the version JSON,
//! > not necessarily the server jar). **E-15 must verify the real check end to end before
//! > `SRV-002` can be claimed.**
//!
//! So there is currently **no trustworthy digest for the Minecraft Java `server.jar`**. Building
//! a "reject a tampered artifact" control against a digest whose source is unresolved would be
//! the exact failure `GAME-006` and `SRV-003` discipline exists to prevent: a security control
//! that reads as authoritative and is not.
//!
//! Therefore:
//!
//! * [`verify`] **is** implemented, because it is mechanism rather than fact: it compares a
//!   stream against an expected digest and refuses on mismatch. The Adoptium JRE path in
//!   `research.md` is `[V]` with SHA-256 verified and 12/12 checks, so the mechanism has a real
//!   caller today.
//! * **Acquisition is not implemented.** There is no HTTP client here and no recorded source
//!   URL to call for the jar. [`verify::ArtifactSource`] is the seam the caller supplies.
//! * [`config`], [`paths`] and [`provision`] need no unresolved fact and are complete.
//!
//! # The parts that carry security weight
//!
//! * [`verify`] refuses **before** execution and never echoes file contents into its error.
//! * [`config`] makes `enable-rcon=false` and `enable-query=false` **structural**: a generated
//!   file cannot contain `true` for either unless a caller explicitly asks *and* supplies a
//!   secret, so `GAME-008` and `SRV-004` cannot be violated by forgetting a flag.
//! * [`paths`] confines every write to the per-user data directory and resolves symlinks and
//!   `..` **before** the comparison, so a crafted name cannot escape (`SRV-010`).
//! * [`provision`] returns an **empty** list today, because no acquisition experiment has
//!   passed. An empty provisionable list is the correct state, not a missing feature (`SRV-003`).

mod config;
mod paths;
mod provision;
mod verify;

pub use config::{
    generate, unverified_mappings, ConfigError, GeneratedConfig, ServerConfigRequest,
    MAX_CONFIG_BYTES, MIN_SECRET_LEN,
};
pub use paths::{DataPaths, PathError};
pub use provision::{AcquisitionRecord, ProvisionError, ProvisionableSet};
pub use verify::{verify, ArtifactSource, Digest, VerifyError};
