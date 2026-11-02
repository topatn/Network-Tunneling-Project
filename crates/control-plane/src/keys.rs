//! Control-plane signing-key management, `kid` rotation with overlap window, and relay
//! verification key distribution (`T-050`, `SEC-016`, `SEC-004`, `contracts.md` §§2, 7).
//!
//! ## Rotation and Overlap-Window Rule (`SEC-016`)
//!
//! To rotate signing keys with zero downtime for connected or starting tunnels:
//!
//! 1. Every newly issued allocation token has a bounded lifetime (`token_ttl_secs`), and relays
//!    poll `/internal/v1/relays/{relay_id}/state` on a bounded interval (`state_poll_interval_secs`).
//! 2. When rotating from `kid_old` to `kid_new`, [`RotationPolicy::validate_overlap_window`]
//!    requires `overlap_window_secs >= token_ttl_secs + state_poll_interval_secs + clock_skew_secs`.
//! 3. During `[now, now + overlap_window_secs)`:
//!    - `kid_new` is the sole [`KeyLifecycleState::ActiveSigning`] key used to sign new tokens.
//!    - Both `kid_new` and `kid_old` ([`KeyLifecycleState::VerifyOnlyOverlap`]) are exported in
//!      [`SigningKeyRing::export_verification_keys`] so relays accept in-flight tokens signed
//!      just before the rotation.
//! 4. At `now >= valid_until_epoch_s`, `kid_old` ceases to verify (`RejectCode::TokenInvalid`)
//!    and is omitted from [`SigningKeyRing::export_verification_keys`] and removed by
//!    [`SigningKeyRing::prune_expired`].

pub use gtt_protocol::token::{
    is_valid_kid, sign_with_key, verify_allocation_token, AllocationTokenClaims,
    AllocationTokenHeader, KeyId, KeyLifecycleState, KeyRingError, ManagedSigningKey, RelayKeySet,
    RelayVerificationKey, SigningKeyMaterial, SigningKeyRing, TokenSignError,
    TokenVerificationError, DEFAULT_TOKEN_ISSUER, KEY_LEN_BYTES, MAX_KEYS_IN_RING,
    MAX_OVERLAP_WINDOW_SECS, MAX_TOKEN_LEN, TOKEN_ALG_HS256, TOKEN_TYP_ALLOC,
};

/// Minimum safety margin (clock skew + relay state poll propagation) added to `max_token_ttl_secs`
/// when computing the minimum safe key-rotation overlap window (`SEC-016`).
pub const DEFAULT_ROTATION_SAFETY_MARGIN_SECS: i64 = 120;

/// Policy governing control-plane signing key lifetimes and overlap windows (`T-050`, `SEC-016`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RotationPolicy {
    /// Maximum lifetime of an issued allocation token in seconds (`1..=3600`).
    pub max_token_ttl_secs: i64,
    /// Safety margin in seconds covering relay state-poll propagation and clock skew (`>= 30`).
    pub safety_margin_secs: i64,
    /// Configured overlap window in seconds (`>= max_token_ttl_secs + safety_margin_secs`).
    pub overlap_window_secs: i64,
}

impl RotationPolicy {
    /// Construct and validate a [`RotationPolicy`].
    pub fn new(
        max_token_ttl_secs: i64,
        safety_margin_secs: i64,
        overlap_window_secs: i64,
    ) -> Result<Self, &'static str> {
        if !(1..=3600).contains(&max_token_ttl_secs) {
            return Err("max_token_ttl_secs must be between 1 and 3600 seconds");
        }
        if !(30..=3600).contains(&safety_margin_secs) {
            return Err("safety_margin_secs must be between 30 and 3600 seconds");
        }
        let min_overlap = max_token_ttl_secs.saturating_add(safety_margin_secs);
        if overlap_window_secs < min_overlap || overlap_window_secs > MAX_OVERLAP_WINDOW_SECS {
            return Err(
                "overlap_window_secs must be >= max_token_ttl_secs + safety_margin_secs and <= 86400",
            );
        }
        Ok(Self {
            max_token_ttl_secs,
            safety_margin_secs,
            overlap_window_secs,
        })
    }

    /// Minimum safe overlap window in seconds (`max_token_ttl_secs + safety_margin_secs`).
    pub fn min_safe_overlap_secs(&self) -> i64 {
        self.max_token_ttl_secs
            .saturating_add(self.safety_margin_secs)
    }
}
