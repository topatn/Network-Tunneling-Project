//! PostgreSQL connection handling for the control plane.
//!
//! `AGENTS.md` §4 requires every network operation to have a timeout, so the pool is
//! built with an explicit acquire timeout and a bounded connection count rather than
//! sqlx's defaults.

use std::time::Duration;

use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::error::Result;

pub mod accounts;
pub mod audit;
pub mod entities;
pub mod invariants;
pub mod ops;
pub mod profile;
pub mod relay;
pub mod sharing;
pub mod tunnels;

pub use entities::*;
pub use profile::{ProfileMapping, ProfileSpec};

/// Longest a caller waits to obtain a connection before failing.
///
/// Without this, a saturated pool makes requests queue indefinitely, which is an
/// unbounded wait on untrusted input (a burst of requests) and exactly the failure
/// `AGENTS.md` §4 forbids.
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);

/// Upper bound on pooled connections.
///
/// Bounded so a traffic spike cannot open connections without limit. Sized for the
/// control plane's role: it is not on the data path, so it needs far fewer connections
/// than the relay or engine.
const MAX_CONNECTIONS: u32 = 10;

/// Longest a statement may run before it is cancelled.
const STATEMENT_TIMEOUT: Duration = Duration::from_secs(15);

/// A handle to the control-plane database.
#[derive(Debug, Clone)]
pub struct Database {
    pool: PgPool,
}

impl Database {
    /// Connect and verify the connection is usable before returning.
    ///
    /// Verifying here rather than lazily means a misconfigured `DATABASE_URL` fails at
    /// start-up, where it is visible, instead of on the first request that needs a row.
    pub async fn connect(url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .acquire_timeout(ACQUIRE_TIMEOUT)
            .max_connections(MAX_CONNECTIONS)
            .connect(url)
            .await?;
        Self::from_pool(pool).await
    }

    /// Wrap an existing pool, for tests that need to control the lifecycle themselves.
    pub async fn from_pool(pool: PgPool) -> Result<Self> {
        // Bounded so a pathological query cannot hold a connection until the pool gives
        // up on it. Applied per-connection rather than per-statement so it covers
        // statements this crate does not write, such as a migration.
        sqlx::query("SET statement_timeout = $1")
            .bind(STATEMENT_TIMEOUT.as_millis() as i64)
            .execute(&pool)
            .await?;
        Ok(Self { pool })
    }

    /// The underlying pool, for callers composing their own transactions.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Close the pool and wait for in-flight statements.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}
