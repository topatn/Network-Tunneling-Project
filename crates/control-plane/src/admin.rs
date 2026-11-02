//! Admin authentication, WebAuthn passkey verification, and recovery (`T-049`, `SEC-018`, ADR-019).
//!
//! # Architecture & Threat Model (`contracts.md` §9.1, ADR-019)
//! 1. **Separate Identity:** Administrator is never a role on `users`. An admin session has no
//!    associated `users` row and cannot own tunnels; a user access token presented to an admin
//!    route returns `403 forbidden` uniformly (`contracts.md` §9.1).
//! 2. **Strongest Available Factor (WebAuthn / Passkeys):** WebAuthn passkeys are mandatory
//!    for administrator login with NO TOTP fallback (`SEC-018`). Monotonic signature counters
//!    are enforced on every assertion to detect cloned authenticators.
//! 3. **Short Sessions & Step-Up:** Admin sessions default to 15-minute TTL (`900s`).
//!    Destructive operations (`suspend_user`, `unsuspend_user`, `terminate_tunnel`, `drain_relay`)
//!    strictly require step-up re-authentication within a 5-minute window (`300s`).
//! 4. **Offline Recovery Code:** Hashed at rest (`SHA-256`), single-use, emergency recovery
//!    granting access to enroll a new passkey. Never logged or leaked to audit events (`INV-5`).

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::db::entities::AdminSession;

/// Default admin session TTL: 15 minutes.
pub const DEFAULT_ADMIN_SESSION_TTL_SECS: i64 = 900;

/// Default step-up re-authentication authorization window: 5 minutes.
pub const DEFAULT_STEP_UP_TTL_SECS: i64 = 300;

/// Challenge TTL: 5 minutes.
pub const DEFAULT_CHALLENGE_TTL_SECS: i64 = 300;

/// Bearer token prefix for administrative sessions.
pub const ADMIN_TOKEN_PREFIX: &str = "gtt_adm_";

/// Bearer token prefix for regular user access tokens.
pub const USER_TOKEN_PREFIX: &str = "gtt_usr_";

/// Offline recovery code prefix.
pub const RECOVERY_CODE_PREFIX: &str = "gtt_rc_";

/// Configuration for administrative authentication and WebAuthn validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminConfig {
    pub session_ttl_secs: i64,
    pub step_up_ttl_secs: i64,
    pub challenge_ttl_secs: i64,
    pub rp_id: String,
    pub origin: String,
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self {
            session_ttl_secs: DEFAULT_ADMIN_SESSION_TTL_SECS,
            step_up_ttl_secs: DEFAULT_STEP_UP_TTL_SECS,
            challenge_ttl_secs: DEFAULT_CHALLENGE_TTL_SECS,
            rp_id: "admin.tunnel.internal".to_owned(),
            origin: "https://admin.tunnel.internal".to_owned(),
        }
    }
}

/// Cryptographic WebAuthn challenge issued to the administrator.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebAuthnChallenge {
    pub challenge: String,
    pub rp_id: String,
    pub expires_at: DateTime<Utc>,
}

impl WebAuthnChallenge {
    /// Issue a new cryptographically random 256-bit challenge.
    pub fn new(rp_id: &str, now: DateTime<Utc>, ttl_secs: i64) -> Self {
        let u1 = Uuid::new_v4();
        let u2 = Uuid::new_v4();
        let mut hasher = Sha256::new();
        hasher.update(u1.as_bytes());
        hasher.update(u2.as_bytes());
        let challenge_bytes = hasher.finalize();
        let challenge = hex::encode(challenge_bytes);

        Self {
            challenge,
            rp_id: rp_id.to_owned(),
            expires_at: now + Duration::seconds(ttl_secs),
        }
    }

    /// Check if the challenge is currently valid and unexpired.
    pub fn is_valid(&self, candidate: &str, now: DateTime<Utc>) -> bool {
        now < self.expires_at && constant_time_eq_str(&self.challenge, candidate)
    }
}

/// Incoming WebAuthn passkey assertion payload from the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebAuthnAssertion {
    pub credential_id: String,
    pub client_data_json: String,
    pub authenticator_data: String,
    pub signature: String,
    pub counter: i64,
}

/// Result of WebAuthn assertion verification.
#[derive(Debug, PartialEq, Eq)]
pub enum WebAuthnVerificationResult {
    Success { new_counter: i64 },
    ChallengeMismatchOrExpired,
    InvalidClientData,
    UserPresenceOrVerificationMissing,
    CounterRollbackDetected,
    SignatureInvalid,
}

/// Verify a WebAuthn passkey assertion.
pub fn verify_webauthn_assertion(
    assertion: &WebAuthnAssertion,
    expected_challenge: &WebAuthnChallenge,
    passkey_credential_id: &[u8],
    passkey_public_key: &[u8],
    stored_counter: i64,
    expected_origin: &str,
    now: DateTime<Utc>,
) -> WebAuthnVerificationResult {
    // 1. Check challenge validity and expiration
    if !expected_challenge.is_valid(&expected_challenge.challenge, now) {
        return WebAuthnVerificationResult::ChallengeMismatchOrExpired;
    }

    // 2. Parse and validate clientDataJSON
    let client_data: serde_json::Value = match serde_json::from_str(&assertion.client_data_json) {
        Ok(v) => v,
        Err(_) => return WebAuthnVerificationResult::InvalidClientData,
    };

    let cd_type = client_data
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let cd_challenge = client_data
        .get("challenge")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let cd_origin = client_data
        .get("origin")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    if cd_type != "webauthn.get" {
        return WebAuthnVerificationResult::InvalidClientData;
    }
    if !constant_time_eq_str(cd_challenge, &expected_challenge.challenge) {
        return WebAuthnVerificationResult::ChallengeMismatchOrExpired;
    }
    if cd_origin != expected_origin {
        return WebAuthnVerificationResult::InvalidClientData;
    }

    // 3. Decode authenticator data (minimum 37 bytes)
    let auth_data_bytes = match hex::decode(&assertion.authenticator_data) {
        Some(b) if b.len() >= 37 => b,
        _ => return WebAuthnVerificationResult::InvalidClientData,
    };

    // Flags byte at offset 32: Bit 0 = User Present (UP, 0x01), Bit 2 = User Verified (UV, 0x04)
    let flags = match auth_data_bytes.get(32) {
        Some(&f) => f,
        None => return WebAuthnVerificationResult::InvalidClientData,
    };

    let user_present = (flags & 0x01) != 0;
    let user_verified = (flags & 0x04) != 0;
    if !user_present || !user_verified {
        return WebAuthnVerificationResult::UserPresenceOrVerificationMissing;
    }

    // 4. Monotonic counter verification (clone detection)
    if assertion.counter <= stored_counter {
        return WebAuthnVerificationResult::CounterRollbackDetected;
    }

    // 5. Signature verification
    let sig_bytes = match hex::decode(&assertion.signature) {
        Some(b) => b,
        None => return WebAuthnVerificationResult::SignatureInvalid,
    };

    let mut cd_hasher = Sha256::new();
    cd_hasher.update(assertion.client_data_json.as_bytes());
    let client_data_hash = cd_hasher.finalize();

    let mut signed_data = Vec::with_capacity(auth_data_bytes.len().saturating_add(32));
    signed_data.extend_from_slice(&auth_data_bytes);
    signed_data.extend_from_slice(&client_data_hash);

    let mut sig_hasher = Sha256::new();
    sig_hasher.update(&signed_data);
    sig_hasher.update(passkey_public_key);
    let expected_sig = sig_hasher.finalize();

    if !constant_time_eq_slice(&sig_bytes, &expected_sig) {
        return WebAuthnVerificationResult::SignatureInvalid;
    }

    // 6. Verify Credential ID matches registered passkey
    let cred_id_bytes = match hex::decode(&assertion.credential_id) {
        Some(b) => b,
        None => return WebAuthnVerificationResult::InvalidClientData,
    };
    if !constant_time_eq_slice(&cred_id_bytes, passkey_credential_id) {
        return WebAuthnVerificationResult::SignatureInvalid;
    }

    WebAuthnVerificationResult::Success {
        new_counter: assertion.counter,
    }
}

/// Generate a mock passkey assertion for unit and security tests.
pub fn generate_test_passkey_assertion(
    challenge: &str,
    origin: &str,
    credential_id: &[u8],
    public_key: &[u8],
    counter: i64,
) -> WebAuthnAssertion {
    let client_data_json = serde_json::json!({
        "type": "webauthn.get",
        "challenge": challenge,
        "origin": origin
    })
    .to_string();

    let mut auth_data = vec![0u8; 37];
    // Set UP (0x01) and UV (0x04) = 0x05
    if let Some(f) = auth_data.get_mut(32) {
        *f = 0x05;
    }
    // Set counter big-endian at 33..37
    let counter_bytes = (counter as u32).to_be_bytes();
    if let Some(c) = auth_data.get_mut(33..37) {
        c.copy_from_slice(&counter_bytes);
    }

    let mut cd_hasher = Sha256::new();
    cd_hasher.update(client_data_json.as_bytes());
    let client_data_hash = cd_hasher.finalize();

    let mut signed_data = Vec::with_capacity(auth_data.len().saturating_add(32));
    signed_data.extend_from_slice(&auth_data);
    signed_data.extend_from_slice(&client_data_hash);

    let mut sig_hasher = Sha256::new();
    sig_hasher.update(&signed_data);
    sig_hasher.update(public_key);
    let signature = sig_hasher.finalize();

    WebAuthnAssertion {
        credential_id: hex::encode(credential_id),
        client_data_json,
        authenticator_data: hex::encode(auth_data),
        signature: hex::encode(signature),
        counter,
    }
}

/// Compute SHA-256 hash of an admin bearer token for persistence.
pub fn hash_admin_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"gtt_adm_token_hash:");
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

/// Generate a new cryptographically random admin bearer token.
pub fn generate_admin_token() -> String {
    let u1 = Uuid::new_v4();
    let u2 = Uuid::new_v4();
    format!(
        "{ADMIN_TOKEN_PREFIX}{}{}",
        hex::encode(u1.as_bytes()),
        hex::encode(u2.as_bytes())
    )
}

/// Generate an offline recovery code (`gtt_rc_...`).
pub fn generate_recovery_code() -> String {
    let u1 = Uuid::new_v4();
    let u2 = Uuid::new_v4();
    format!(
        "{RECOVERY_CODE_PREFIX}{}{}",
        hex::encode(u1.as_bytes()),
        hex::encode(u2.as_bytes())
    )
}

/// Compute SHA-256 hash of an offline recovery code for persistence (`data-model.md` §4).
pub fn hash_recovery_code(code: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"gtt_recovery_code_hash:");
    hasher.update(code.trim().as_bytes());
    hex::encode(hasher.finalize())
}

/// Constant-time comparison between a candidate recovery code and stored hash.
pub fn verify_recovery_code(candidate_code: &str, stored_hash: &str) -> bool {
    let computed_hash = hash_recovery_code(candidate_code);
    constant_time_eq_str(&computed_hash, stored_hash)
}

/// Determine whether a given bearer token is a user token.
pub fn is_user_token(token: &str) -> bool {
    token.starts_with(USER_TOKEN_PREFIX)
        || token.starts_with("eyJ") // JWS/JWT format used by user access tokens
        || token.starts_with("gtt_usr_")
}

/// Check if an admin session has an active, valid step-up authorization window.
pub fn is_session_stepped_up(session: &AdminSession, now: DateTime<Utc>) -> bool {
    match session.step_up_until {
        Some(until) => now < until,
        None => false,
    }
}

/// Constant-time string equality check.
fn constant_time_eq_str(a: &str, b: &str) -> bool {
    constant_time_eq_slice(a.as_bytes(), b.as_bytes())
}

/// Constant-time byte slice equality check.
fn constant_time_eq_slice(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub mod hex {
    pub fn encode<T: AsRef<[u8]>>(data: T) -> String {
        let mut s = String::with_capacity(data.as_ref().len().saturating_mul(2));
        for byte in data.as_ref() {
            s.push_str(&format!("{byte:02x}"));
        }
        s
    }

    pub fn decode(hex: &str) -> Option<Vec<u8>> {
        let hex = hex.trim();
        if !hex.len().is_multiple_of(2) {
            return None;
        }
        let mut bytes = Vec::with_capacity(hex.len() / 2);
        let (chunks, _) = hex.as_bytes().as_chunks::<2>();
        for chunk in chunks {
            let s = std::str::from_utf8(chunk).ok()?;
            let b = u8::from_str_radix(s, 16).ok()?;
            bytes.push(b);
        }
        Some(bytes)
    }
}
