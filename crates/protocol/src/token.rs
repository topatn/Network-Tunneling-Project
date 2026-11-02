//! Allocation token claims, compact signing/verification, `kid` key-ring management, and
//! zero-downtime overlap rotation (`contracts.md` §§2, 7, 10, `SEC-004`, `SEC-016`, T-050, T-060).
//!
//! ## Claims and Wire Format (`contracts.md` §2)
//!
//! An allocation token is a compact three-part ASCII string
//! `<base64url(header)>.<base64url(claims)>.<base64url(tag)>` bounded to [`MAX_TOKEN_LEN`]
//! (`4096` bytes, matching `contracts/start-tunnel-response.schema.json`).
//!
//! * **Header ([`AllocationTokenHeader`]):** `alg` (`"HS256"`), `kid` (`^[A-Za-z0-9_-]{1,64}$`),
//!   and `typ` (`"gtt-alloc+jwt"`).
//! * **Claims ([`AllocationTokenClaims`]):** `iss`, `aud` (relay UUID), `sub` (tunnel UUID),
//!   `aid` (allocation UUID), `mappings` (`Vec<HelloAckMapping>`), `iat`, `exp`, and `jti` (UUID).
//! * **Verification (`SEC-004`):** Verifies length cap, header `alg`/`typ`, `kid` lookup in the
//!   relay's active [`RelayKeySet`] (rejecting unknown or post-overlap expired `kid` values),
//!   constant-time RFC 2104 HMAC-SHA256 tag comparison, `INV-1` local-target absence, claim
//!   structural rules, `iss`, `aud` (must equal the verifying relay's own `relay_id`), and `exp`
//!   against the current timestamp.

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::control::{HelloAckMapping, RejectCode};
use crate::http::MAX_TUNNEL_MAPPINGS;
use crate::validate::{
    is_valid_identifier, is_valid_uuid, scan_json_for_local_target, ValidationError,
};

/// Maximum length of an encoded allocation token (`contracts/start-tunnel-response.schema.json`).
pub const MAX_TOKEN_LEN: usize = 4096;

/// Expected token algorithm identifier.
pub const TOKEN_ALG_HS256: &str = "HS256";

/// Expected token type header.
pub const TOKEN_TYP_ALLOC: &str = "gtt-alloc+jwt";

/// Default control-plane token issuer (`iss`).
pub const DEFAULT_TOKEN_ISSUER: &str = "gtt-control-plane";

/// Required signing key length in bytes (256 bits).
pub const KEY_LEN_BYTES: usize = 32;

/// Maximum overlap window for a rotated key (`24 hours` in seconds).
pub const MAX_OVERLAP_WINDOW_SECS: i64 = 86_400;

/// Maximum number of concurrent keys retained in a [`SigningKeyRing`].
pub const MAX_KEYS_IN_RING: usize = 16;

/// Validated key identifier (`kid`, `^[A-Za-z0-9_-]{1,64}$`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyId(String);

impl KeyId {
    /// Validate and construct a [`KeyId`].
    pub fn new(raw: impl Into<String>) -> Result<Self, KeyRingError> {
        let s = raw.into();
        if !is_valid_kid(&s) {
            return Err(KeyRingError::InvalidKeyId);
        }
        Ok(Self(s))
    }

    /// Borrow the `kid` string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Check whether `s` is a valid `kid` (`1..=64` ASCII `[A-Za-z0-9_-]`).
pub fn is_valid_kid(s: &str) -> bool {
    let bytes = s.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
}

/// 256-bit signing key material.
///
/// Custom `Debug` implementation redacts the raw bytes so key material never leaks to logs
/// (`PRIV-002`, `AGENTS.md` §7).
#[derive(Clone)]
pub struct SigningKeyMaterial([u8; KEY_LEN_BYTES]);

impl SigningKeyMaterial {
    /// Construct [`SigningKeyMaterial`] from a 32-byte array.
    pub fn from_bytes(bytes: [u8; KEY_LEN_BYTES]) -> Result<Self, KeyRingError> {
        // Reject an all-zero key so uninitialized buffers cannot become signing keys.
        if bytes.iter().all(|b| *b == 0) {
            return Err(KeyRingError::WeakKeyMaterial);
        }
        Ok(Self(bytes))
    }

    /// Borrow the raw 32-byte key material.
    pub fn as_bytes(&self) -> &[u8; KEY_LEN_BYTES] {
        &self.0
    }
}

impl fmt::Debug for SigningKeyMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SigningKeyMaterial(<redacted>)")
    }
}

impl PartialEq for SigningKeyMaterial {
    fn eq(&self, other: &Self) -> bool {
        constant_time_eq_32(&self.0, &other.0)
    }
}

impl Eq for SigningKeyMaterial {}

/// Lifecycle state of a signing key in [`SigningKeyRing`] (`SEC-016`, T-050).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyLifecycleState {
    /// Primary key used to sign newly issued tokens and accepted for verification.
    ActiveSigning {
        /// Unix timestamp (seconds) when this key became the active signing key.
        activated_at_epoch_s: i64,
    },
    /// Rotated key inside its overlap window: no longer signs new tokens, but continues to
    /// verify existing tokens until `valid_until_epoch_s`.
    VerifyOnlyOverlap {
        /// Unix timestamp (seconds) when this key was superseded by a newer key.
        retired_at_epoch_s: i64,
        /// Unix timestamp (seconds) when the overlap window ends and the key stops verifying.
        valid_until_epoch_s: i64,
    },
}

impl KeyLifecycleState {
    /// True if this key is valid for verifying tokens at `now_epoch_s`.
    pub fn can_verify_at(self, now_epoch_s: i64) -> bool {
        match self {
            Self::ActiveSigning { .. } => true,
            Self::VerifyOnlyOverlap {
                valid_until_epoch_s,
                ..
            } => now_epoch_s < valid_until_epoch_s,
        }
    }
}

/// One managed key entry inside [`SigningKeyRing`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedSigningKey {
    /// Key identifier (`kid`).
    pub kid: KeyId,
    /// Secret key material (redacted in `Debug`).
    pub material: SigningKeyMaterial,
    /// Current rotation state.
    pub state: KeyLifecycleState,
}

/// Verification key entry distributed to relays via `GET /internal/v1/relays/{relay_id}/state`
/// (`contracts.md` §7, T-050).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayVerificationKey {
    /// Key identifier (`kid`).
    pub kid: KeyId,
    /// Verification key material (redacted in `Debug`).
    pub material: SigningKeyMaterial,
    /// Optional overlap-window expiration (`None` for the current active signing key,
    /// `Some(epoch_s)` for a key in its overlap window).
    pub valid_until_epoch_s: Option<i64>,
}

/// Snapshot of verification keys held by a relay (`contracts.md` §7, T-050, T-060).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelayKeySet {
    keys: Vec<RelayVerificationKey>,
}

impl RelayKeySet {
    /// Construct a [`RelayKeySet`] from distributed verification keys.
    pub fn new(keys: Vec<RelayVerificationKey>) -> Self {
        Self { keys }
    }

    /// Borrow all keys in the set.
    pub fn keys(&self) -> &[RelayVerificationKey] {
        &self.keys
    }

    /// Look up a key by `kid` that is valid at `now_epoch_s`.
    pub fn find_valid_key(&self, kid: &str, now_epoch_s: i64) -> Option<&RelayVerificationKey> {
        self.keys.iter().find(|k| {
            k.kid.as_str() == kid
                && match k.valid_until_epoch_s {
                    None => true,
                    Some(until) => now_epoch_s < until,
                }
        })
    }
}

/// Error returned by [`SigningKeyRing`] operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyRingError {
    /// `kid` did not match `^[A-Za-z0-9_-]{1,64}$`.
    InvalidKeyId,
    /// Key material was all-zero.
    WeakKeyMaterial,
    /// `kid` already exists in the key ring.
    DuplicateKeyId(String),
    /// Overlap window was `<= 0` or exceeded [`MAX_OVERLAP_WINDOW_SECS`].
    InvalidOverlapWindow {
        /// Supplied overlap window in seconds.
        overlap_window_secs: i64,
    },
    /// Key ring reached [`MAX_KEYS_IN_RING`] without pruning expired keys.
    RingCapacityExceeded,
    /// Attempted to revoke the sole active signing key without rotating first.
    CannotRevokeActiveSigningKey,
}

impl fmt::Display for KeyRingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidKeyId => f.write_str("kid must match ^[A-Za-z0-9_-]{1,64}$"),
            Self::WeakKeyMaterial => f.write_str("signing key material must not be all-zero"),
            Self::DuplicateKeyId(kid) => write!(f, "duplicate kid {kid:?} in key ring"),
            Self::InvalidOverlapWindow {
                overlap_window_secs,
            } => write!(
                f,
                "overlap_window_secs ({overlap_window_secs}) must be in 1..={MAX_OVERLAP_WINDOW_SECS}"
            ),
            Self::RingCapacityExceeded => {
                write!(f, "key ring capacity ({MAX_KEYS_IN_RING}) exceeded")
            }
            Self::CannotRevokeActiveSigningKey => {
                f.write_str("cannot revoke the active signing key without rotating to a new key first")
            }
        }
    }
}

impl std::error::Error for KeyRingError {}

/// Control-plane signing key ring supporting zero-downtime overlap rotation (`SEC-016`, T-050).
#[derive(Debug, Clone)]
pub struct SigningKeyRing {
    issuer: String,
    keys: Vec<ManagedSigningKey>,
}

impl SigningKeyRing {
    /// Initialize a [`SigningKeyRing`] with its first active signing key.
    pub fn new(
        issuer: impl Into<String>,
        initial_kid: KeyId,
        initial_material: SigningKeyMaterial,
        now_epoch_s: i64,
    ) -> Self {
        Self {
            issuer: issuer.into(),
            keys: vec![ManagedSigningKey {
                kid: initial_kid,
                material: initial_material,
                state: KeyLifecycleState::ActiveSigning {
                    activated_at_epoch_s: now_epoch_s,
                },
            }],
        }
    }

    /// Borrow the configured issuer (`iss`).
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// Return the current active signing key.
    pub fn active_signing_key(&self) -> Option<&ManagedSigningKey> {
        self.keys
            .iter()
            .find(|k| matches!(k.state, KeyLifecycleState::ActiveSigning { .. }))
    }

    /// Rotate to a new active signing key while keeping the previous active key in
    /// [`KeyLifecycleState::VerifyOnlyOverlap`] for `overlap_window_secs` (`SEC-016`, T-050).
    ///
    /// Automatically prunes any prior overlap keys whose overlap window has already elapsed at
    /// `now_epoch_s`.
    pub fn rotate(
        &mut self,
        new_kid: KeyId,
        new_material: SigningKeyMaterial,
        now_epoch_s: i64,
        overlap_window_secs: i64,
    ) -> Result<(), KeyRingError> {
        if !(1..=MAX_OVERLAP_WINDOW_SECS).contains(&overlap_window_secs) {
            return Err(KeyRingError::InvalidOverlapWindow {
                overlap_window_secs,
            });
        }
        self.prune_expired(now_epoch_s);

        if self.keys.iter().any(|k| k.kid == new_kid) {
            return Err(KeyRingError::DuplicateKeyId(new_kid.as_str().to_owned()));
        }
        if self.keys.len() >= MAX_KEYS_IN_RING {
            return Err(KeyRingError::RingCapacityExceeded);
        }

        let valid_until_epoch_s = now_epoch_s.saturating_add(overlap_window_secs);
        for key in &mut self.keys {
            if matches!(key.state, KeyLifecycleState::ActiveSigning { .. }) {
                key.state = KeyLifecycleState::VerifyOnlyOverlap {
                    retired_at_epoch_s: now_epoch_s,
                    valid_until_epoch_s,
                };
            }
        }

        self.keys.push(ManagedSigningKey {
            kid: new_kid,
            material: new_material,
            state: KeyLifecycleState::ActiveSigning {
                activated_at_epoch_s: now_epoch_s,
            },
        });
        Ok(())
    }

    /// Remove all `VerifyOnlyOverlap` keys whose overlap window has elapsed (`valid_until <= now`).
    pub fn prune_expired(&mut self, now_epoch_s: i64) -> usize {
        let before = self.keys.len();
        self.keys.retain(|k| k.state.can_verify_at(now_epoch_s));
        before.saturating_sub(self.keys.len())
    }

    /// Emergency revocation of a non-active key by `kid` (e.g. after rotating away from a
    /// suspected compromised key so the old key is immediately dropped without waiting for its
    /// overlap window).
    pub fn emergency_revoke_key(&mut self, kid: &str) -> Result<bool, KeyRingError> {
        if let Some(active) = self.active_signing_key() {
            if active.kid.as_str() == kid {
                return Err(KeyRingError::CannotRevokeActiveSigningKey);
            }
        }
        let before = self.keys.len();
        self.keys.retain(|k| k.kid.as_str() != kid);
        Ok(self.keys.len() < before)
    }

    /// Export the verification key set for distribution to relays at `now_epoch_s`
    /// (`contracts.md` §7).
    pub fn export_verification_keys(&self, now_epoch_s: i64) -> RelayKeySet {
        let exported = self
            .keys
            .iter()
            .filter(|k| k.state.can_verify_at(now_epoch_s))
            .map(|k| RelayVerificationKey {
                kid: k.kid.clone(),
                material: k.material.clone(),
                valid_until_epoch_s: match k.state {
                    KeyLifecycleState::ActiveSigning { .. } => None,
                    KeyLifecycleState::VerifyOnlyOverlap {
                        valid_until_epoch_s,
                        ..
                    } => Some(valid_until_epoch_s),
                },
            })
            .collect();
        RelayKeySet::new(exported)
    }

    /// Sign an [`AllocationTokenClaims`] payload using the current `ActiveSigning` key.
    pub fn sign_allocation_token(
        &self,
        claims: &AllocationTokenClaims,
    ) -> Result<String, TokenSignError> {
        claims.validate().map_err(TokenSignError::InvalidClaims)?;
        if claims.iss != self.issuer {
            return Err(TokenSignError::IssuerMismatch {
                expected: self.issuer.clone(),
                actual: claims.iss.clone(),
            });
        }
        let active = self
            .active_signing_key()
            .ok_or(TokenSignError::NoActiveSigningKey)?;
        sign_with_key(&active.kid, &active.material, claims)
    }
}

/// Protected header of an allocation token (`contracts.md` §2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocationTokenHeader {
    /// Algorithm identifier (`"HS256"`).
    pub alg: String,
    /// Key identifier (`kid`) for key rotation (`contracts.md` §2).
    pub kid: String,
    /// Token type (`"gtt-alloc+jwt"`).
    pub typ: String,
}

/// Claims carried inside a signed allocation token (`contracts.md` §2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocationTokenClaims {
    /// Token issuer (`iss`).
    pub iss: String,
    /// Target relay UUID (`aud`).
    pub aud: String,
    /// Tunnel UUID (`sub`).
    pub sub: String,
    /// Allocation UUID (`aid`).
    pub aid: String,
    /// Authorized port mappings (`index`, `mapping_key`, `protocol`, `public_port`).
    pub mappings: Vec<HelloAckMapping>,
    /// Issued-at Unix timestamp in seconds (`iat`).
    pub iat: i64,
    /// Expiration Unix timestamp in seconds (`exp`).
    pub exp: i64,
    /// Unique token identifier (`jti`, UUID) for replay prevention.
    pub jti: String,
}

impl AllocationTokenClaims {
    /// Validate `contracts.md` §2 claim rules.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.iss.trim().is_empty() || self.iss.len() > 64 {
            return Err(ValidationError::InvalidField {
                field: "iss",
                issue: "must be 1-64 characters",
            });
        }
        if !is_valid_uuid(&self.aud) {
            return Err(ValidationError::InvalidField {
                field: "aud",
                issue: "must be a valid relay UUID",
            });
        }
        if !is_valid_uuid(&self.sub) {
            return Err(ValidationError::InvalidField {
                field: "sub",
                issue: "must be a valid tunnel UUID",
            });
        }
        if !is_valid_uuid(&self.aid) {
            return Err(ValidationError::InvalidField {
                field: "aid",
                issue: "must be a valid allocation UUID",
            });
        }
        if !is_valid_uuid(&self.jti) {
            return Err(ValidationError::InvalidField {
                field: "jti",
                issue: "must be a valid token UUID",
            });
        }
        if self.iat <= 0 || self.exp <= self.iat {
            return Err(ValidationError::InvalidField {
                field: "iat/exp",
                issue: "iat must be positive and exp must be greater than iat",
            });
        }
        if !(1..=MAX_TUNNEL_MAPPINGS).contains(&self.mappings.len()) {
            return Err(ValidationError::InvalidField {
                field: "mappings",
                issue: "must contain 1 to 8 mappings",
            });
        }
        for (idx, m) in self.mappings.iter().enumerate() {
            if m.index > 7 {
                return Err(ValidationError::InvalidField {
                    field: "mappings.index",
                    issue: "must be 0..=7",
                });
            }
            if !is_valid_identifier(&m.mapping_key) {
                return Err(ValidationError::InvalidField {
                    field: "mappings.mapping_key",
                    issue: "must match ^[a-z][a-z0-9-]{0,31}$",
                });
            }
            if m.public_port == 0 {
                return Err(ValidationError::InvalidField {
                    field: "mappings.public_port",
                    issue: "must be 1..=65535",
                });
            }
            if let Some(earlier) = self.mappings.get(..idx) {
                if earlier
                    .iter()
                    .any(|prev| prev.index == m.index || prev.mapping_key == m.mapping_key)
                {
                    return Err(ValidationError::InvalidField {
                        field: "mappings",
                        issue: "mapping index and mapping_key must be unique",
                    });
                }
            }
        }
        Ok(())
    }
}

/// Error returned when signing an allocation token fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenSignError {
    /// Claims failed validation.
    InvalidClaims(ValidationError),
    /// Claims `iss` did not match the key ring's issuer.
    IssuerMismatch {
        /// Key ring's issuer.
        expected: String,
        /// Claims' issuer.
        actual: String,
    },
    /// Key ring had no active signing key.
    NoActiveSigningKey,
    /// Serialized token exceeded [`MAX_TOKEN_LEN`].
    EncodedTokenTooLong,
}

impl fmt::Display for TokenSignError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidClaims(e) => write!(f, "invalid token claims: {e}"),
            Self::IssuerMismatch { expected, actual } => {
                write!(f, "token issuer {actual:?} does not match {expected:?}")
            }
            Self::NoActiveSigningKey => f.write_str("no active signing key in key ring"),
            Self::EncodedTokenTooLong => {
                f.write_str("encoded allocation token exceeded 4096 bytes")
            }
        }
    }
}

impl std::error::Error for TokenSignError {}

/// Reason an allocation token was rejected during relay verification (`SEC-004`, T-060).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenVerificationError {
    /// Token was empty, exceeded [`MAX_TOKEN_LEN`], did not have 3 parts, or failed base64url/JSON decoding.
    MalformedToken(&'static str),
    /// Header `kid` was unknown or its overlap window had already expired at `now_epoch_s`.
    UnknownOrExpiredKeyId,
    /// Cryptographic tag did not match the signing key for `kid`.
    InvalidSignature,
    /// Claims violated `INV-1` or structural bounds.
    InvalidClaims(&'static str),
    /// Claims `iss` did not match the expected control-plane issuer.
    WrongIssuer,
    /// Claims `aud` did not match this relay's `relay_id`.
    WrongAudience,
    /// Token `exp <= now_epoch_s`.
    TokenExpired,
}

impl TokenVerificationError {
    /// Map this verification failure to the wire [`RejectCode`] specified in `contracts.md` §10
    /// (`SEC-004`, T-060).
    pub fn to_reject_code(&self) -> RejectCode {
        match self {
            Self::TokenExpired => RejectCode::TokenExpired,
            Self::MalformedToken(_)
            | Self::UnknownOrExpiredKeyId
            | Self::InvalidSignature
            | Self::InvalidClaims(_)
            | Self::WrongIssuer
            | Self::WrongAudience => RejectCode::TokenInvalid,
        }
    }
}

impl fmt::Display for TokenVerificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedToken(reason) => write!(f, "malformed allocation token: {reason}"),
            Self::UnknownOrExpiredKeyId => f.write_str("unknown or expired signing key id (kid)"),
            Self::InvalidSignature => f.write_str("allocation token signature verification failed"),
            Self::InvalidClaims(reason) => write!(f, "invalid allocation token claims: {reason}"),
            Self::WrongIssuer => f.write_str("allocation token issuer mismatch"),
            Self::WrongAudience => f.write_str("allocation token audience (aud) mismatch"),
            Self::TokenExpired => f.write_str("allocation token has expired"),
        }
    }
}

impl std::error::Error for TokenVerificationError {}

/// Sign an [`AllocationTokenClaims`] structure with a specific `(kid, material)` pair.
pub fn sign_with_key(
    kid: &KeyId,
    material: &SigningKeyMaterial,
    claims: &AllocationTokenClaims,
) -> Result<String, TokenSignError> {
    claims.validate().map_err(TokenSignError::InvalidClaims)?;
    let header = AllocationTokenHeader {
        alg: TOKEN_ALG_HS256.to_owned(),
        kid: kid.as_str().to_owned(),
        typ: TOKEN_TYP_ALLOC.to_owned(),
    };
    let header_bytes =
        serde_json::to_vec(&header).map_err(|_| TokenSignError::EncodedTokenTooLong)?;
    let claims_bytes =
        serde_json::to_vec(claims).map_err(|_| TokenSignError::EncodedTokenTooLong)?;

    let header_b64 = base64url_encode(&header_bytes);
    let claims_b64 = base64url_encode(&claims_bytes);
    let signing_input = format!("{header_b64}.{claims_b64}");
    let tag = hmac_sha256(material.as_bytes(), signing_input.as_bytes());
    let tag_b64 = base64url_encode(&tag);

    let token = format!("{signing_input}.{tag_b64}");
    if token.len() > MAX_TOKEN_LEN {
        return Err(TokenSignError::EncodedTokenTooLong);
    }
    Ok(token)
}

/// Verify a compact allocation token against `key_set`, `expected_issuer`, `expected_relay_aud`,
/// and `now_epoch_s` (`contracts.md` §2, `SEC-004`).
pub fn verify_allocation_token(
    token: &str,
    key_set: &RelayKeySet,
    expected_issuer: &str,
    expected_relay_aud: &str,
    now_epoch_s: i64,
) -> Result<(AllocationTokenHeader, AllocationTokenClaims), TokenVerificationError> {
    if token.is_empty() || token.len() > MAX_TOKEN_LEN {
        return Err(TokenVerificationError::MalformedToken(
            "token length out of bounds",
        ));
    }

    let mut parts = token.split('.');
    let header_b64 = parts
        .next()
        .ok_or(TokenVerificationError::MalformedToken("missing header"))?;
    let claims_b64 = parts
        .next()
        .ok_or(TokenVerificationError::MalformedToken("missing claims"))?;
    let tag_b64 = parts
        .next()
        .ok_or(TokenVerificationError::MalformedToken("missing signature"))?;
    if parts.next().is_some() {
        return Err(TokenVerificationError::MalformedToken(
            "token must have exactly three dot-separated parts",
        ));
    }

    let header_bytes = base64url_decode(header_b64, 512).ok_or(
        TokenVerificationError::MalformedToken("invalid header base64url"),
    )?;
    let header: AllocationTokenHeader = serde_json::from_slice(&header_bytes)
        .map_err(|_| TokenVerificationError::MalformedToken("invalid header JSON"))?;

    if header.alg != TOKEN_ALG_HS256 || header.typ != TOKEN_TYP_ALLOC || !is_valid_kid(&header.kid)
    {
        return Err(TokenVerificationError::MalformedToken(
            "unsupported header alg, typ, or kid format",
        ));
    }

    let verification_key = key_set
        .find_valid_key(&header.kid, now_epoch_s)
        .ok_or(TokenVerificationError::UnknownOrExpiredKeyId)?;

    let tag_bytes = base64url_decode(tag_b64, 64).ok_or(TokenVerificationError::MalformedToken(
        "invalid signature base64url",
    ))?;
    if tag_bytes.len() != KEY_LEN_BYTES {
        return Err(TokenVerificationError::InvalidSignature);
    }
    let mut provided_tag = [0u8; KEY_LEN_BYTES];
    provided_tag.copy_from_slice(&tag_bytes);

    let signing_input = format!("{header_b64}.{claims_b64}");
    let expected_tag = hmac_sha256(
        verification_key.material.as_bytes(),
        signing_input.as_bytes(),
    );
    if !constant_time_eq_32(&provided_tag, &expected_tag) {
        return Err(TokenVerificationError::InvalidSignature);
    }

    let claims_bytes = base64url_decode(claims_b64, MAX_TOKEN_LEN).ok_or(
        TokenVerificationError::MalformedToken("invalid claims base64url"),
    )?;
    let claims_value: serde_json::Value = serde_json::from_slice(&claims_bytes)
        .map_err(|_| TokenVerificationError::MalformedToken("invalid claims JSON"))?;

    // INV-1: reject any token whose payload smuggles a local destination field.
    scan_json_for_local_target(&claims_value).map_err(|_| {
        TokenVerificationError::InvalidClaims("forbidden local target field (INV-1)")
    })?;

    let claims: AllocationTokenClaims = serde_json::from_value(claims_value)
        .map_err(|_| TokenVerificationError::InvalidClaims("claims schema mismatch"))?;
    claims
        .validate()
        .map_err(|_| TokenVerificationError::InvalidClaims("claims field validation failed"))?;

    if claims.iss != expected_issuer {
        return Err(TokenVerificationError::WrongIssuer);
    }
    if claims.aud != expected_relay_aud {
        return Err(TokenVerificationError::WrongAudience);
    }
    if now_epoch_s >= claims.exp {
        return Err(TokenVerificationError::TokenExpired);
    }

    Ok((header, claims))
}

/// Compute SHA-256 digest of `bytes` (`NET-001` hash-verified transfer helper).
pub fn sha256_digest(bytes: &[u8]) -> [u8; 32] {
    let hash = Sha256::digest(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&hash);
    out
}

/// Compute RFC 2104 HMAC-SHA256 using `sha2::Sha256`.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK_SIZE: usize = 64;
    let mut key_block = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let hashed = Sha256::digest(key);
        for (dst, src) in key_block.iter_mut().zip(hashed.iter()) {
            *dst = *src;
        }
    } else {
        for (dst, src) in key_block.iter_mut().zip(key.iter()) {
            *dst = *src;
        }
    }

    let mut ipad = [0x36u8; BLOCK_SIZE];
    let mut opad = [0x5cu8; BLOCK_SIZE];
    for (i, k) in key_block.iter().enumerate() {
        if let Some(ip) = ipad.get_mut(i) {
            *ip ^= *k;
        }
        if let Some(op) = opad.get_mut(i) {
            *op ^= *k;
        }
    }

    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_hash);
    let outer_hash = outer.finalize();

    let mut out = [0u8; 32];
    out.copy_from_slice(&outer_hash);
    out
}

/// Constant-time comparison of two 32-byte arrays (`SEC-004`).
pub fn constant_time_eq_32(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

const B64URL_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Encode bytes as RFC 4648 §5 base64url without padding.
pub fn base64url_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().saturating_mul(4).div_ceil(3));
    let (chunks, rem) = input.as_chunks::<3>();
    for &[b0, b1, b2] in chunks {
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        push_b64_char(&mut out, ((n >> 18) & 0x3F) as usize);
        push_b64_char(&mut out, ((n >> 12) & 0x3F) as usize);
        push_b64_char(&mut out, ((n >> 6) & 0x3F) as usize);
        push_b64_char(&mut out, (n & 0x3F) as usize);
    }
    if rem.len() == 1 {
        let b0 = rem.first().copied().unwrap_or(0);
        let n = u32::from(b0) << 16;
        push_b64_char(&mut out, ((n >> 18) & 0x3F) as usize);
        push_b64_char(&mut out, ((n >> 12) & 0x3F) as usize);
    } else if rem.len() == 2 {
        let b0 = rem.first().copied().unwrap_or(0);
        let b1 = rem.get(1).copied().unwrap_or(0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8);
        push_b64_char(&mut out, ((n >> 18) & 0x3F) as usize);
        push_b64_char(&mut out, ((n >> 12) & 0x3F) as usize);
        push_b64_char(&mut out, ((n >> 6) & 0x3F) as usize);
    }
    out
}

fn push_b64_char(out: &mut String, idx: usize) {
    if let Some(&ch) = B64URL_ALPHABET.get(idx) {
        out.push(char::from(ch));
    }
}

/// Decode RFC 4648 §5 base64url (without padding), enforcing `max_decoded_bytes` before allocation.
pub fn base64url_decode(encoded: &str, max_decoded_bytes: usize) -> Option<Vec<u8>> {
    let bytes = encoded.as_bytes();
    if bytes.len() % 4 == 1 {
        return None;
    }
    let decoded_len = (bytes.len() / 4) * 3
        + match bytes.len() % 4 {
            2 => 1,
            3 => 2,
            _ => 0,
        };
    if decoded_len > max_decoded_bytes {
        return None;
    }

    let mut out = Vec::with_capacity(decoded_len);
    let (chunks, rem) = bytes.as_chunks::<4>();
    for &[b0, b1, b2, b3] in chunks {
        let c0 = decode_b64_byte(b0)?;
        let c1 = decode_b64_byte(b1)?;
        let c2 = decode_b64_byte(b2)?;
        let c3 = decode_b64_byte(b3)?;
        let n =
            (u32::from(c0) << 18) | (u32::from(c1) << 12) | (u32::from(c2) << 6) | u32::from(c3);
        out.push(((n >> 16) & 0xFF) as u8);
        out.push(((n >> 8) & 0xFF) as u8);
        out.push((n & 0xFF) as u8);
    }
    if rem.len() == 2 {
        let c0 = decode_b64_byte(*rem.first()?)?;
        let c1 = decode_b64_byte(*rem.get(1)?)?;
        if (c1 & 0x0F) != 0 {
            return None;
        }
        let n = (u32::from(c0) << 18) | (u32::from(c1) << 12);
        out.push(((n >> 16) & 0xFF) as u8);
    } else if rem.len() == 3 {
        let c0 = decode_b64_byte(*rem.first()?)?;
        let c1 = decode_b64_byte(*rem.get(1)?)?;
        let c2 = decode_b64_byte(*rem.get(2)?)?;
        if (c2 & 0x03) != 0 {
            return None;
        }
        let n = (u32::from(c0) << 18) | (u32::from(c1) << 12) | (u32::from(c2) << 6);
        out.push(((n >> 16) & 0xFF) as u8);
        out.push(((n >> 8) & 0xFF) as u8);
    }
    Some(out)
}

fn decode_b64_byte(b: u8) -> Option<u8> {
    match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'-' => Some(62),
        b'_' => Some(63),
        _ => None,
    }
}
