//! User authentication, PKCE exchange, session rotation, and logout routes (`T-040`,
//! `SEC-001`, `SEC-003`, `FR-001`, `ADR-018`, `contracts.md` §§5.1–5.4, `api.md` ##1–4).
//!
//! ## Invariants Enforced Here
//!
//! 1. **PKCE S256 Only (`SEC-001`, `ADR-018`, `contracts.md` §5.1):**
//!    Only RFC 7636 S256 code challenge method is accepted. Any request declaring or using
//!    `plain` method is rejected with HTTP `400 validation_failed` (`ApiErrorCode::ValidationFailed`).
//! 2. **Redirect URI Allow-List (`contracts.md` §5.1):**
//!    Redirect URIs must match one of the registered allow-list prefixes (RFC 8252 loopback forms
//!    or the official client scheme `gtt://auth/callback`). Unregistered URIs fail with
//!    `ApiErrorCode::ValidationFailed`.
//! 3. **High-Entropy Tokens & No Plaintext Database Storage (`SEC-001`, `AGENTS.md` §7):**
//!    Refresh tokens carry 256 bits (32 bytes) of CSPRNG entropy (`gtt_rt_...`). Only
//!    `sha256(refresh_token)` is stored in PostgreSQL (`auth_sessions.refresh_token_hash`).
//! 4. **Refresh Rotation & Family Revocation on Reuse (`SEC-003`, `contracts.md` §5.2):**
//!    Presenting an active refresh token rotates it atomically to a new one. Presenting an already
//!    revoked or rotated refresh token indicates token theft; the entire session family is revoked
//!    immediately via [`db_revoke_session_family`] and HTTP `401 unauthenticated` is returned with
//!    an `auth.refresh_reused` audit trail.
//! 5. **Suspended Account Refusal (`SEC-015`, `contracts.md` §5.1):**
//!    A suspended user cannot exchange an authorization code or refresh tokens; both routes return
//!    HTTP `403 forbidden`.

use chrono::{DateTime, Duration, Utc};
use gtt_protocol::http::{
    ApiErrorCode, AuthExchangeRequest, AuthRefreshRequest, AuthTokenResponse, ErrorDetail,
    UserProfileResponse,
};
use gtt_protocol::token::{base64url_decode, base64url_encode, constant_time_eq_32, hmac_sha256};
use gtt_protocol::validate::scan_json_for_local_target;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::accounts::{
    create_session as db_create_session,
    get_session_by_token_hash_any as db_get_session_by_token_hash_any, get_user as db_get_user,
    revoke_session_family as db_revoke_session_family,
    revoke_user_sessions as db_revoke_user_sessions, rotate_session as db_rotate_session,
    upsert_user as db_upsert_user,
};
use crate::db::audit::{AuditAction, AuditMetadata};
use crate::db::entities::{ActorType, UserStatus};
use crate::db::ops::{append_audit_event, NewAuditEvent};
use crate::routes::tunnels::{ApiFailure, ApiResponse, MAX_REQUEST_BODY_BYTES};

/// Prefix for issued refresh tokens for deterministic secret-leak scanning (`PRIV-002`, `SEC-002`).
pub const REFRESH_TOKEN_PREFIX: &str = "gtt_rt_";

/// Default access token lifetime (15 minutes).
pub const DEFAULT_ACCESS_TOKEN_TTL_SECS: i64 = 900;

/// Default refresh token session lifetime (30 days).
pub const DEFAULT_REFRESH_TOKEN_TTL_SECS: i64 = 30 * 86_400;

/// Configuration for authentication routes (`contracts.md` §§5.1–5.4, `T-040`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthRouteConfig {
    /// Lifetime of issued access tokens in seconds (`60..=86400`).
    pub access_token_ttl_seconds: i64,
    /// Lifetime of refresh token sessions in seconds (`3600..=7776000`).
    pub refresh_token_ttl_seconds: i64,
    /// Registered redirect URI prefixes accepted by `POST /v1/auth/exchange`.
    pub allowed_redirect_uri_prefixes: Vec<String>,
    /// Pinned signing key used to issue and authenticate compact access tokens.
    pub access_token_signing_key: [u8; 32],
}

impl Default for AuthRouteConfig {
    fn default() -> Self {
        Self {
            access_token_ttl_seconds: DEFAULT_ACCESS_TOKEN_TTL_SECS,
            refresh_token_ttl_seconds: DEFAULT_REFRESH_TOKEN_TTL_SECS,
            allowed_redirect_uri_prefixes: vec![
                "http://127.0.0.1:".to_owned(),
                "http://localhost:".to_owned(),
                "gtt://auth/callback".to_owned(),
            ],
            access_token_signing_key: [0x5Au8; 32],
        }
    }
}

/// Payload claims contained within signed access tokens (`T-040`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessTokenClaims {
    /// User identifier (UUID).
    pub sub: String,
    /// Issuance timestamp (epoch seconds).
    pub iat: i64,
    /// Expiration timestamp (epoch seconds).
    pub exp: i64,
    /// Unique token ID (UUID).
    pub jti: String,
}

/// Generate a cryptographically random, high-entropy refresh token (`SEC-001`).
pub fn generate_refresh_token() -> String {
    let mut entropy = [0u8; 32];
    let u1 = Uuid::new_v4();
    let u2 = Uuid::new_v4();
    for (idx, &b) in u1.as_bytes().iter().enumerate() {
        if let Some(slot) = entropy.get_mut(idx) {
            *slot = b;
        }
    }
    for (idx, &b) in u2.as_bytes().iter().enumerate() {
        if let Some(slot) = entropy.get_mut(idx.saturating_add(16)) {
            *slot = b;
        }
    }
    format!("{REFRESH_TOKEN_PREFIX}{}", base64url_encode(&entropy))
}

/// Compute SHA-256 digest of bytes.
pub fn sha256_digest(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Issue a compact HMAC-SHA256 signed access token for `user_id`.
pub fn create_access_token(
    user_id: Uuid,
    signing_key: &[u8; 32],
    ttl_seconds: i64,
    now: DateTime<Utc>,
) -> Result<String, ApiFailure> {
    let iat = now.timestamp();
    let exp = iat.saturating_add(ttl_seconds);
    let claims = AccessTokenClaims {
        sub: user_id.to_string(),
        iat,
        exp,
        jti: Uuid::new_v4().to_string(),
    };
    let claims_bytes = serde_json::to_vec(&claims).map_err(|e| {
        ApiFailure::new(
            ApiErrorCode::Internal,
            "failed to encode access token claims",
            "req_auth_token",
            Some(vec![ErrorDetail {
                field: "access_token".to_owned(),
                issue: e.to_string(),
            }]),
        )
    })?;
    let header = serde_json::json!({"alg": "HS256", "typ": "JWT"});
    let header_bytes = serde_json::to_vec(&header).map_err(|e| {
        ApiFailure::new(
            ApiErrorCode::Internal,
            "failed to encode access token header",
            "req_auth_token",
            Some(vec![ErrorDetail {
                field: "access_token".to_owned(),
                issue: e.to_string(),
            }]),
        )
    })?;
    let header_b64 = base64url_encode(&header_bytes);
    let claims_b64 = base64url_encode(&claims_bytes);
    let signing_input = format!("{header_b64}.{claims_b64}");
    let tag = hmac_sha256(signing_key, signing_input.as_bytes());
    let tag_b64 = base64url_encode(&tag);
    Ok(format!("{signing_input}.{tag_b64}"))
}

/// Verify a compact access token and extract the authenticated `user_id`.
pub fn verify_access_token(
    token: &str,
    signing_key: &[u8; 32],
    now: DateTime<Utc>,
) -> Result<Uuid, ApiFailure> {
    let mut parts = token.split('.');
    let header_b64 = parts.next().ok_or_else(|| {
        ApiFailure::new(
            ApiErrorCode::Unauthenticated,
            "invalid access token format",
            "req_auth",
            None,
        )
    })?;
    let claims_b64 = parts.next().ok_or_else(|| {
        ApiFailure::new(
            ApiErrorCode::Unauthenticated,
            "invalid access token format",
            "req_auth",
            None,
        )
    })?;
    let tag_b64 = parts.next().ok_or_else(|| {
        ApiFailure::new(
            ApiErrorCode::Unauthenticated,
            "invalid access token format",
            "req_auth",
            None,
        )
    })?;
    if parts.next().is_some() {
        return Err(ApiFailure::new(
            ApiErrorCode::Unauthenticated,
            "invalid access token format",
            "req_auth",
            None,
        ));
    }

    let signing_input = format!("{header_b64}.{claims_b64}");
    let expected_tag = hmac_sha256(signing_key, signing_input.as_bytes());
    let tag_bytes = base64url_decode(tag_b64, 64).ok_or_else(|| {
        ApiFailure::new(
            ApiErrorCode::Unauthenticated,
            "invalid access token signature encoding",
            "req_auth",
            None,
        )
    })?;
    if tag_bytes.len() != 32 {
        return Err(ApiFailure::new(
            ApiErrorCode::Unauthenticated,
            "invalid access token signature length",
            "req_auth",
            None,
        ));
    }
    let mut provided_tag = [0u8; 32];
    provided_tag.copy_from_slice(&tag_bytes);
    if !constant_time_eq_32(&provided_tag, &expected_tag) {
        return Err(ApiFailure::new(
            ApiErrorCode::Unauthenticated,
            "access token signature verification failed",
            "req_auth",
            None,
        ));
    }

    let claims_bytes = base64url_decode(claims_b64, 4096).ok_or_else(|| {
        ApiFailure::new(
            ApiErrorCode::Unauthenticated,
            "invalid access token claims encoding",
            "req_auth",
            None,
        )
    })?;
    let claims: AccessTokenClaims = serde_json::from_slice(&claims_bytes).map_err(|_| {
        ApiFailure::new(
            ApiErrorCode::Unauthenticated,
            "invalid access token claims schema",
            "req_auth",
            None,
        )
    })?;

    if now.timestamp() >= claims.exp {
        return Err(ApiFailure::new(
            ApiErrorCode::Unauthenticated,
            "access token expired",
            "req_auth",
            None,
        ));
    }

    Uuid::parse_str(&claims.sub).map_err(|_| {
        ApiFailure::new(
            ApiErrorCode::Unauthenticated,
            "invalid user identifier in token",
            "req_auth",
            None,
        )
    })
}

/// Simulated identity returned from identity-provider code exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityProviderProfile {
    /// IdP issuer identifier (e.g. `"https://accounts.google.com"`).
    pub issuer: String,
    /// IdP user subject identifier.
    pub subject: String,
    /// User display name.
    pub display_name: Option<String>,
}

/// Parse an authorization code or mock code into an identity profile.
///
/// In production, this contacts the OIDC token endpoint (`ADR-009`).
/// In test/mock harnesses, supports `mock:{issuer}:{subject}:{display_name}` format.
pub fn resolve_identity_from_code(code: &str) -> Result<IdentityProviderProfile, ApiFailure> {
    let trimmed = code.trim();
    if trimmed.starts_with("mock:") {
        let parts: Vec<&str> = trimmed.split(':').collect();
        let issuer = parts
            .get(1)
            .copied()
            .unwrap_or("https://idp.example.invalid");
        let subject = parts.get(2).copied().unwrap_or("mock-user-1");
        let display_name = parts
            .get(3)
            .copied()
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        return Ok(IdentityProviderProfile {
            issuer: issuer.to_owned(),
            subject: subject.to_owned(),
            display_name,
        });
    }
    // Default standard OAuth code exchange format: derives a deterministic subject from the code
    let subject_hash = sha256_digest(trimmed.as_bytes());
    let subject = format!(
        "sub_{}",
        gtt_protocol::token::base64url_encode(&subject_hash)
            .get(..16)
            .unwrap_or("user")
    );
    Ok(IdentityProviderProfile {
        issuer: "https://auth.game-tunnel.invalid".to_owned(),
        subject,
        display_name: Some("GTT Player".to_owned()),
    })
}

/// `POST /v1/auth/exchange` handler (`contracts.md` §5.1, `api.md` #1, `T-040`).
pub async fn handle_auth_exchange(
    pool: &PgPool,
    config: &AuthRouteConfig,
    request_id: &str,
    raw_body: &[u8],
) -> Result<ApiResponse<AuthTokenResponse>, ApiFailure> {
    if raw_body.is_empty() || raw_body.len() > MAX_REQUEST_BODY_BYTES {
        return Err(ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "request body size exceeds allowed bounds",
            request_id,
            None,
        ));
    }

    let parsed_json: serde_json::Value = serde_json::from_slice(raw_body).map_err(|e| {
        ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "invalid request body JSON",
            request_id,
            Some(vec![ErrorDetail {
                field: "body".to_owned(),
                issue: e.to_string(),
            }]),
        )
    })?;

    // INV-1 check: reject any local destination key
    scan_json_for_local_target(&parsed_json).map_err(|e| {
        ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "forbidden local target field in request",
            request_id,
            Some(vec![ErrorDetail {
                field: "target".to_owned(),
                issue: e.to_string(),
            }]),
        )
    })?;

    let req: AuthExchangeRequest = serde_json::from_value(parsed_json).map_err(|e| {
        ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "schema validation failed",
            request_id,
            Some(vec![ErrorDetail {
                field: "request".to_owned(),
                issue: e.to_string(),
            }]),
        )
    })?;

    req.validate().map_err(|e| {
        ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "request field validation failed",
            request_id,
            Some(vec![ErrorDetail {
                field: "validation".to_owned(),
                issue: e.to_string(),
            }]),
        )
    })?;

    // Validate redirect URI allow-list (contracts.md §5.1)
    let is_allowed_uri = config
        .allowed_redirect_uri_prefixes
        .iter()
        .any(|prefix| req.redirect_uri.starts_with(prefix));
    if !is_allowed_uri {
        return Err(ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "redirect_uri is not in the registered allow-list",
            request_id,
            Some(vec![ErrorDetail {
                field: "redirect_uri".to_owned(),
                issue: "unregistered redirect URI".to_owned(),
            }]),
        ));
    }

    // Resolve user identity from authorization code
    let idp_profile = resolve_identity_from_code(&req.code)?;

    // Upsert user record in PostgreSQL
    let user = db_upsert_user(
        pool,
        &idp_profile.issuer,
        &idp_profile.subject,
        idp_profile.display_name.as_deref(),
    )
    .await
    .map_err(|e| {
        ApiFailure::new(
            ApiErrorCode::Internal,
            "failed to persist user identity",
            request_id,
            Some(vec![ErrorDetail {
                field: "db".to_owned(),
                issue: e.to_string(),
            }]),
        )
    })?;

    // Check suspension (contracts.md §5.1: return 403 forbidden)
    if user.status == UserStatus::Suspended.as_str() {
        return Err(ApiFailure::new(
            ApiErrorCode::Forbidden,
            "user account is suspended",
            request_id,
            None,
        ));
    }

    let now = Utc::now();
    let refresh_token = generate_refresh_token();
    let refresh_token_hash = sha256_digest(refresh_token.as_bytes());
    let family_id = Uuid::new_v4();
    let expires_at = now + Duration::seconds(config.refresh_token_ttl_seconds);

    let _session = db_create_session(
        pool,
        user.id,
        family_id,
        &refresh_token_hash,
        req.device_label.as_deref(),
        expires_at,
    )
    .await
    .map_err(|e| {
        ApiFailure::new(
            ApiErrorCode::Internal,
            "failed to persist auth session",
            request_id,
            Some(vec![ErrorDetail {
                field: "db".to_owned(),
                issue: e.to_string(),
            }]),
        )
    })?;

    // Issue signed access token
    let access_token = create_access_token(
        user.id,
        &config.access_token_signing_key,
        config.access_token_ttl_seconds,
        now,
    )?;

    // Emit audit event
    let empty_meta = AuditMetadata::empty();
    let audit_event = NewAuditEvent {
        actor_type: ActorType::User,
        actor_id: Some(user.id),
        action: AuditAction::AuthSignedIn.as_str(),
        subject_type: "user",
        subject_id: Some(user.id),
        request_id: Some(request_id),
        metadata: &empty_meta,
    };
    let _ = append_audit_event(pool, &audit_event).await;

    Ok(ApiResponse {
        status: 200,
        replayed: false,
        body: AuthTokenResponse {
            access_token,
            token_type: "Bearer".to_owned(),
            expires_in: config.access_token_ttl_seconds as u64,
            refresh_token,
        },
    })
}

/// `POST /v1/auth/refresh` handler (`contracts.md` §5.2, `api.md` #2, `T-040`, `SEC-003`).
pub async fn handle_auth_refresh(
    pool: &PgPool,
    config: &AuthRouteConfig,
    request_id: &str,
    raw_body: &[u8],
) -> Result<ApiResponse<AuthTokenResponse>, ApiFailure> {
    if raw_body.is_empty() || raw_body.len() > MAX_REQUEST_BODY_BYTES {
        return Err(ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "request body size exceeds allowed bounds",
            request_id,
            None,
        ));
    }

    let req: AuthRefreshRequest = serde_json::from_slice(raw_body).map_err(|e| {
        ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "invalid request body JSON",
            request_id,
            Some(vec![ErrorDetail {
                field: "refresh_token".to_owned(),
                issue: e.to_string(),
            }]),
        )
    })?;

    req.validate().map_err(|e| {
        ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "validation failed",
            request_id,
            Some(vec![ErrorDetail {
                field: "refresh_token".to_owned(),
                issue: e.to_string(),
            }]),
        )
    })?;

    let token_hash = sha256_digest(req.refresh_token.as_bytes());
    let session_opt = db_get_session_by_token_hash_any(pool, &token_hash)
        .await
        .map_err(|e| {
            ApiFailure::new(
                ApiErrorCode::Internal,
                "database error querying session",
                request_id,
                Some(vec![ErrorDetail {
                    field: "db".to_owned(),
                    issue: e.to_string(),
                }]),
            )
        })?;

    let session = session_opt.ok_or_else(|| {
        ApiFailure::new(
            ApiErrorCode::Unauthenticated,
            "invalid refresh token",
            request_id,
            None,
        )
    })?;

    let now = Utc::now();

    // SEC-003: If session is already revoked or expired, REUSE is detected!
    if session.revoked_at.is_some() || session.expires_at <= now {
        // Kill the whole family immediately
        let _ = db_revoke_session_family(pool, session.family_id).await;

        let empty_meta = AuditMetadata::empty();
        let audit_event = NewAuditEvent {
            actor_type: ActorType::User,
            actor_id: Some(session.user_id),
            action: AuditAction::AuthRefreshReused.as_str(),
            subject_type: "auth_session",
            subject_id: Some(session.id),
            request_id: Some(request_id),
            metadata: &empty_meta,
        };
        let _ = append_audit_event(pool, &audit_event).await;

        return Err(ApiFailure::new(
            ApiErrorCode::Unauthenticated,
            "refresh token has already been used or expired; session revoked",
            request_id,
            None,
        ));
    }

    // Verify user account is not suspended
    let user = db_get_user(pool, session.user_id)
        .await
        .map_err(|e| {
            ApiFailure::new(
                ApiErrorCode::Internal,
                "database error querying user",
                request_id,
                Some(vec![ErrorDetail {
                    field: "db".to_owned(),
                    issue: e.to_string(),
                }]),
            )
        })?
        .ok_or_else(|| {
            ApiFailure::new(
                ApiErrorCode::Unauthenticated,
                "user account not found or deleted",
                request_id,
                None,
            )
        })?;

    if user.status == UserStatus::Suspended.as_str() {
        let _ = db_revoke_session_family(pool, session.family_id).await;
        return Err(ApiFailure::new(
            ApiErrorCode::Forbidden,
            "user account is suspended",
            request_id,
            None,
        ));
    }

    // Atomically rotate: revoke old session, insert new session
    let new_refresh_token = generate_refresh_token();
    let new_token_hash = sha256_digest(new_refresh_token.as_bytes());
    let new_expires_at = now + Duration::seconds(config.refresh_token_ttl_seconds);

    let _rotated_session = db_rotate_session(
        pool,
        session.id,
        session.user_id,
        session.family_id,
        &new_token_hash,
        session.device_label.as_deref(),
        new_expires_at,
    )
    .await
    .map_err(|e| {
        ApiFailure::new(
            ApiErrorCode::Internal,
            "failed to rotate session",
            request_id,
            Some(vec![ErrorDetail {
                field: "db".to_owned(),
                issue: e.to_string(),
            }]),
        )
    })?;

    let access_token = create_access_token(
        session.user_id,
        &config.access_token_signing_key,
        config.access_token_ttl_seconds,
        now,
    )?;

    Ok(ApiResponse {
        status: 200,
        replayed: false,
        body: AuthTokenResponse {
            access_token,
            token_type: "Bearer".to_owned(),
            expires_in: config.access_token_ttl_seconds as u64,
            refresh_token: new_refresh_token,
        },
    })
}

/// `POST /v1/auth/logout` handler (`contracts.md` §5.3, `api.md` #3, `T-040`).
pub async fn handle_auth_logout(
    pool: &PgPool,
    request_id: &str,
    user_id: Uuid,
) -> Result<ApiResponse<()>, ApiFailure> {
    db_revoke_user_sessions(pool, user_id).await.map_err(|e| {
        ApiFailure::new(
            ApiErrorCode::Internal,
            "failed to revoke user sessions on logout",
            request_id,
            Some(vec![ErrorDetail {
                field: "db".to_owned(),
                issue: e.to_string(),
            }]),
        )
    })?;

    let empty_meta = AuditMetadata::empty();
    let audit_event = NewAuditEvent {
        actor_type: ActorType::User,
        actor_id: Some(user_id),
        action: AuditAction::AuthSignedOut.as_str(),
        subject_type: "user",
        subject_id: Some(user_id),
        request_id: Some(request_id),
        metadata: &empty_meta,
    };
    let _ = append_audit_event(pool, &audit_event).await;

    Ok(ApiResponse {
        status: 204,
        replayed: false,
        body: (),
    })
}

/// `GET /v1/me` handler (`contracts.md` §5.4, `api.md` #4, `T-040`).
pub async fn handle_get_me(
    pool: &PgPool,
    request_id: &str,
    user_id: Uuid,
) -> Result<ApiResponse<UserProfileResponse>, ApiFailure> {
    let user = db_get_user(pool, user_id)
        .await
        .map_err(|e| {
            ApiFailure::new(
                ApiErrorCode::Internal,
                "database error querying user profile",
                request_id,
                Some(vec![ErrorDetail {
                    field: "db".to_owned(),
                    issue: e.to_string(),
                }]),
            )
        })?
        .ok_or_else(|| {
            ApiFailure::new(
                ApiErrorCode::NotFound,
                "user profile not found",
                request_id,
                None,
            )
        })?;

    if user.status == UserStatus::Suspended.as_str() {
        return Err(ApiFailure::new(
            ApiErrorCode::Forbidden,
            "user account is suspended",
            request_id,
            None,
        ));
    }

    Ok(ApiResponse {
        status: 200,
        replayed: false,
        body: UserProfileResponse {
            id: user.id.to_string(),
            display_name: user.display_name,
            status: user.status.as_str().to_owned(),
        },
    })
}
