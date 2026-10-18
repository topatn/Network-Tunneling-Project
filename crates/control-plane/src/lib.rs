//! HTTP API, authorization policy, and persistence.
//!
//! Routes are defined in `contracts.md` §5 and inventoried in `api.md`. Implemented from
//! T-040.
//!
//! ## What exists here today (T-034)
//!
//! The persistence layer and the invariant checks, and nothing else. There is no HTTP
//! server, no request routing, and no authorization policy: those are T-040 and T-041.
//! The layer is built so those can be added without changing anything below it — every
//! owner-scoped function here takes the owner id and returns `None` for rows the caller
//! may not see, so the policy layer decides what that means (`not_found` per
//! `contracts.md`, not `forbidden`).

pub mod db;
pub mod error;

pub use db::Database;
pub use error::{PersistenceError, Rejection, Result};
