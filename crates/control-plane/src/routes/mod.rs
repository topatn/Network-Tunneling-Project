//! Control-plane route handlers (`api.md`, `contracts.md` §5).
//!
//! Each route handler enforces layered validation (`data-model.md` §7):
//! wire schema validation (`gtt-protocol`), authorization policy (`crate::policy`),
//! idempotency (`contracts.md` §8), and persistence invariants (`crate::db`).

pub mod admin;
pub mod auth;
pub mod invitations;
pub mod relay_internal;
pub mod tunnels;
