//! Rate limiting, account quotas, and user suspension handling (`T-045`, `SEC-008`,
//! `SEC-015`, `contracts.md` §§3, 4).
//!
//! ## Rate-Limit Classes (`contracts.md` §4)
//!
//! * [`RateLimitClass::Strict`] — unauthenticated `POST /v1/invitations/claim` and
//!   `POST /v1/auth/refresh`, keyed by observed client IP plus a global ceiling.
//! * [`RateLimitClass::Normal`] — authenticated tunnel and invitation mutations
//!   (`create_tunnel`, `start_tunnel`, `stop_tunnel`, `issue_tunnel_token`,
//!   `create_invitation`, `revoke_invitation`), keyed by user ID plus a global ceiling.
//! * [`RateLimitClass::Lenient`] — authenticated read routes (`list_tunnels`, `get_tunnel`,
//!   `list_invitations`, `list_admissions`, `usage`), keyed by user ID plus a global ceiling.
//!
//! Every internal collection is strictly bounded by [`RateLimitConfig::max_tracked_keys`]
//! (`AGENTS.md` §4).

use std::collections::BTreeMap;

use sqlx::PgPool;
use uuid::Uuid;

use gtt_protocol::http::ApiErrorCode;

use crate::db::accounts::set_user_status;
use crate::db::audit;
use crate::db::entities::{ActorType, UserStatus};
use crate::db::ops::{append_audit_event, NewAuditEvent};
use crate::policy::{decide, Actor, Decision, ErrorCode as PolicyErrorCode, Operation, Target};
use crate::routes::tunnels::ApiFailure;

/// Rate-limit class from `contracts.md` §4 (`SEC-008`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RateLimitClass {
    /// Unauthenticated `POST /v1/invitations/claim` and `POST /v1/auth/refresh`.
    Strict,
    /// Authenticated tunnel and invitation mutations.
    Normal,
    /// Authenticated read routes.
    Lenient,
}

/// Per-window limit and duration for one [`RateLimitClass`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitBucketConfig {
    /// Maximum allowed requests within `window_seconds` (`>= 1`).
    pub max_requests: u32,
    /// Fixed window duration in seconds (`1..=86400`).
    pub window_seconds: i64,
}

/// Complete rate-limiter configuration (`contracts.md` §4, `SEC-008`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitConfig {
    /// Strict bucket (unauthenticated claim and token refresh).
    pub strict: RateLimitBucketConfig,
    /// Normal bucket (tunnel and invitation mutations).
    pub normal: RateLimitBucketConfig,
    /// Lenient bucket (authenticated reads).
    pub lenient: RateLimitBucketConfig,
    /// Global ceiling across all keys within a class window (`>= 1`).
    pub global_ceiling_per_window: u32,
    /// Maximum number of distinct `(class, key)` entries held in memory (`1..=65536`).
    pub max_tracked_keys: usize,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            strict: RateLimitBucketConfig {
                max_requests: 10,
                window_seconds: 60,
            },
            normal: RateLimitBucketConfig {
                max_requests: 30,
                window_seconds: 60,
            },
            lenient: RateLimitBucketConfig {
                max_requests: 120,
                window_seconds: 60,
            },
            global_ceiling_per_window: 1000,
            max_tracked_keys: 4096,
        }
    }
}

impl RateLimitConfig {
    /// Return the bucket configuration for `class`.
    pub fn bucket_for(&self, class: RateLimitClass) -> RateLimitBucketConfig {
        match class {
            RateLimitClass::Strict => self.strict,
            RateLimitClass::Normal => self.normal,
            RateLimitClass::Lenient => self.lenient,
        }
    }
}

/// Outcome of a rate-limit check (`contracts.md` §§3, 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitDecision {
    /// Request is permitted; `remaining` is the remaining quota in the current window.
    Allowed {
        /// Remaining requests before this key or global window is exhausted.
        remaining: u32,
    },
    /// Request exceeded the per-key or global limit; caller must return HTTP `429`
    /// `rate_limited` with `Retry-After: <retry_after_seconds>`.
    RateLimited {
        /// Positive `Retry-After` header value in seconds (`>= 1`).
        retry_after_seconds: u32,
    },
}

/// HTTP `429 rate_limited` failure pairing the [`ApiFailure`] envelope with the required
/// `Retry-After` seconds (`contracts.md` §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitFailure {
    /// Standard HTTP 429 `rate_limited` response envelope.
    pub failure: ApiFailure,
    /// Value for the HTTP `Retry-After` response header in seconds (`>= 1`).
    pub retry_after_seconds: u32,
}

impl RateLimitDecision {
    /// Convert a [`RateLimitDecision`] into a `Result`, producing a `429 rate_limited`
    /// [`RateLimitFailure`] when limited.
    pub fn into_result(self, request_id: &str) -> Result<u32, RateLimitFailure> {
        match self {
            Self::Allowed { remaining } => Ok(remaining),
            Self::RateLimited {
                retry_after_seconds,
            } => Err(RateLimitFailure {
                failure: ApiFailure::new(
                    ApiErrorCode::RateLimited,
                    "Rate limit exceeded. Retry after the indicated interval.",
                    request_id,
                    None,
                ),
                retry_after_seconds,
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WindowCounter {
    window_start_epoch_s: i64,
    window_end_epoch_s: i64,
    count: u32,
}

/// Bounded in-memory rate limiter enforcing per-key and global ceilings (`SEC-008`).
#[derive(Debug, Clone)]
pub struct RateLimiter {
    config: RateLimitConfig,
    per_key: BTreeMap<(RateLimitClass, String), WindowCounter>,
    global: BTreeMap<RateLimitClass, WindowCounter>,
}

impl RateLimiter {
    /// Create a new [`RateLimiter`] with the given [`RateLimitConfig`].
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            per_key: BTreeMap::new(),
            global: BTreeMap::new(),
        }
    }

    /// Borrow the active [`RateLimitConfig`].
    pub fn config(&self) -> &RateLimitConfig {
        &self.config
    }

    /// Number of currently tracked `(class, key)` counters.
    pub fn tracked_key_count(&self) -> usize {
        self.per_key.len()
    }

    /// Check and record one request for `(class, key)` at `now_epoch_s`.
    ///
    /// `key` is truncated to at most 128 bytes so an untrusted caller cannot bloat the key
    /// table (`AGENTS.md` §4).
    pub fn check_and_record(
        &mut self,
        class: RateLimitClass,
        key: &str,
        now_epoch_s: i64,
    ) -> RateLimitDecision {
        let bucket = self.config.bucket_for(class);
        let window_secs = bucket.window_seconds.clamp(1, 86_400);
        let max_req = bucket.max_requests.max(1);
        let global_max = self.config.global_ceiling_per_window.max(1);
        let max_keys = self.config.max_tracked_keys.clamp(1, 65_536);

        let bounded_key: String = key.chars().take(128).collect();

        // 1. Check global ceiling for this class first before mutating per-key state.
        let global_counter = self.global.entry(class).or_insert(WindowCounter {
            window_start_epoch_s: now_epoch_s,
            window_end_epoch_s: now_epoch_s.saturating_add(window_secs),
            count: 0,
        });
        if now_epoch_s >= global_counter.window_end_epoch_s {
            global_counter.window_start_epoch_s = now_epoch_s;
            global_counter.window_end_epoch_s = now_epoch_s.saturating_add(window_secs);
            global_counter.count = 0;
        }
        if global_counter.count >= global_max {
            let retry_after = u32::try_from(
                global_counter
                    .window_end_epoch_s
                    .saturating_sub(now_epoch_s)
                    .max(1),
            )
            .unwrap_or(1);
            return RateLimitDecision::RateLimited {
                retry_after_seconds: retry_after,
            };
        }

        // 2. Evict expired counters (and oldest counter if still full) when adding a new key.
        let map_key = (class, bounded_key);
        if !self.per_key.contains_key(&map_key) && self.per_key.len() >= max_keys {
            self.per_key
                .retain(|_, v| now_epoch_s < v.window_end_epoch_s);
            if self.per_key.len() >= max_keys {
                let oldest_key = self
                    .per_key
                    .iter()
                    .min_by_key(|(_, v)| v.window_start_epoch_s)
                    .map(|(k, _)| k.clone());
                if let Some(k) = oldest_key {
                    self.per_key.remove(&k);
                }
            }
        }

        let entry = self.per_key.entry(map_key).or_insert(WindowCounter {
            window_start_epoch_s: now_epoch_s,
            window_end_epoch_s: now_epoch_s.saturating_add(window_secs),
            count: 0,
        });
        if now_epoch_s >= entry.window_end_epoch_s {
            entry.window_start_epoch_s = now_epoch_s;
            entry.window_end_epoch_s = now_epoch_s.saturating_add(window_secs);
            entry.count = 0;
        }

        if entry.count >= max_req {
            let retry_after =
                u32::try_from(entry.window_end_epoch_s.saturating_sub(now_epoch_s).max(1))
                    .unwrap_or(1);
            return RateLimitDecision::RateLimited {
                retry_after_seconds: retry_after,
            };
        }

        entry.count = entry.count.saturating_add(1);
        global_counter.count = global_counter.count.saturating_add(1);

        let key_remaining = max_req.saturating_sub(entry.count);
        let global_remaining = global_max.saturating_sub(global_counter.count);
        RateLimitDecision::Allowed {
            remaining: key_remaining.min(global_remaining),
        }
    }
}

/// Account quota configuration (`SEC-015`, `contracts.md` §§3, 5.6, 5.11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaConfig {
    /// Maximum concurrent live (`created` or `active`) tunnels per user.
    pub max_live_tunnels_per_user: i64,
    /// Maximum active (unrevoked, unexpired) invitations per tunnel.
    pub max_active_invitations_per_tunnel: i64,
    /// Maximum relayed bandwidth per user per 24-hour window in bytes.
    pub max_bandwidth_bytes_per_day: i64,
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self {
            max_live_tunnels_per_user: 5,
            max_active_invitations_per_tunnel: 20,
            max_bandwidth_bytes_per_day: 50 * 1024 * 1024 * 1024, // 50 GiB
        }
    }
}

/// Enforce an account quota ceiling, returning HTTP `403 quota_exceeded` when
/// `current_usage >= limit` (`SEC-015`, `contracts.md` §3).
pub fn enforce_quota(
    current_usage: i64,
    limit: i64,
    message: &'static str,
    request_id: &str,
) -> Result<(), ApiFailure> {
    if current_usage >= limit.max(0) {
        return Err(ApiFailure::new(
            ApiErrorCode::QuotaExceeded,
            message,
            request_id,
            None,
        ));
    }
    Ok(())
}

/// Suspend a user account (`SEC-015`, `T-045`).
///
/// Requires [`Actor::Admin`]. Atomically marks the user `suspended`, terminates all of their
/// `created`/`active` tunnels, releases any live `relay_allocations` and `allocation_ports`,
/// revokes all `auth_sessions`, and appends an `audit_events` row (`admin.user_suspended`,
/// `INV-5`).
pub async fn suspend_user_account(
    pool: &PgPool,
    actor: Actor,
    target_user_id: Uuid,
    reason: Option<&str>,
    request_id: &str,
) -> Result<u64, ApiFailure> {
    if let Decision::Deny(code) = decide(actor, Operation::SuspendUser, Target::None) {
        let (api_code, msg) = match code {
            PolicyErrorCode::Unauthenticated => {
                (ApiErrorCode::Unauthenticated, "Authentication required.")
            }
            PolicyErrorCode::Forbidden => (
                ApiErrorCode::Forbidden,
                "Only an administrator may suspend a user account.",
            ),
            PolicyErrorCode::NotFound => (ApiErrorCode::NotFound, "User not found."),
            PolicyErrorCode::ValidationFailed => {
                (ApiErrorCode::ValidationFailed, "Invalid request.")
            }
            PolicyErrorCode::Conflict => (ApiErrorCode::Conflict, "State conflict."),
        };
        return Err(ApiFailure::new(api_code, msg, request_id, None));
    }

    let affected = set_user_status(pool, target_user_id, UserStatus::Suspended, reason)
        .await
        .map_err(|_| {
            ApiFailure::new(
                ApiErrorCode::Internal,
                "Database error while suspending user.",
                request_id,
                None,
            )
        })?;

    if affected == 0 {
        return Err(ApiFailure::new(
            ApiErrorCode::NotFound,
            "User not found.",
            request_id,
            None,
        ));
    }

    let reason_str = reason.unwrap_or("admin_suspended");
    let safe_reason: String = reason_str.chars().take(128).collect();
    let metadata = audit::build([
        (
            "state_after".to_owned(),
            serde_json::Value::String("suspended".to_owned()),
        ),
        ("reason".to_owned(), serde_json::Value::String(safe_reason)),
    ])
    .map_err(|_| {
        ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Suspension reason failed audit metadata validation (INV-5).",
            request_id,
            None,
        )
    })?;

    append_audit_event(
        pool,
        &NewAuditEvent {
            actor_type: ActorType::Admin,
            actor_id: None,
            action: "admin.user_suspended",
            subject_type: "user",
            subject_id: Some(target_user_id),
            request_id: Some(request_id),
            metadata: &metadata,
        },
    )
    .await
    .map_err(|_| {
        ApiFailure::new(
            ApiErrorCode::Internal,
            "Failed to record user suspension audit event.",
            request_id,
            None,
        )
    })?;

    Ok(affected)
}
