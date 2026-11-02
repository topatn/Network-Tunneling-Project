//! Administrative HTTP routes (`/admin/v1`, `T-049`, `SEC-018`, `SEC-015`, ADR-019).
//!
//! # Invariants Enforced
//! 1. User access tokens are strictly forbidden on all `/admin/v1/*` routes (`contracts.md` §9.1).
//! 2. Destructive routes (`suspend`, `unsuspend`, `terminate`, `drain`) strictly require
//!    step-up re-authentication within the active step-up window.
//! 3. All administrative actions write append-only audit events (`actor_type = 'admin'`).
//! 4. Recovery codes and secret credentials never appear in logs or audit events (`INV-5`).

use chrono::{DateTime, Duration, Utc};
use gtt_protocol::http::ApiErrorCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::admin::{
    generate_admin_token, hash_admin_token, is_session_stepped_up, is_user_token,
    verify_recovery_code, verify_webauthn_assertion, AdminConfig, WebAuthnAssertion,
    WebAuthnChallenge, WebAuthnVerificationResult,
};
use crate::db::accounts::set_user_status;
use crate::db::admin::{
    consume_recovery_code, create_admin_session, get_admin, get_admin_by_username,
    get_admin_session_by_token_hash, update_admin_session_step_up, update_passkey_counter,
};
use crate::db::audit::{AuditAction, AuditMetadata};
use crate::db::entities::{ActorType, AdminSession, RelayStatus, UserStatus};
use crate::db::ops::{append_audit_event, NewAuditEvent};
use crate::db::relay::set_relay_status;
use crate::db::tunnels::terminate_tunnel as db_terminate_tunnel;
use crate::routes::tunnels::{ApiFailure, ApiResponse};

/// Maximum admin request body size: 64 KiB.
pub const MAX_ADMIN_REQUEST_BODY_BYTES: usize = 65_536;

#[derive(Debug, Serialize, Deserialize)]
pub struct AdminChallengeRequest {
    pub username: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AdminLoginRequest {
    pub username: String,
    pub assertion: WebAuthnAssertion,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AdminStepUpRequest {
    pub assertion: WebAuthnAssertion,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AdminRecoveryRequest {
    pub username: String,
    pub recovery_code: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AdminSuspendRequest {
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AdminTerminateRequest {
    pub reason: Option<String>,
}

/// Authentication context for an administrative request.
pub enum AdminAuthContext {
    Authenticated {
        session: AdminSession,
        admin_id: Uuid,
    },
    UserTokenForbidden,
    Unauthenticated,
}

fn ok_response<T: Serialize>(body: T) -> ApiResponse<Value> {
    ApiResponse {
        status: 200,
        replayed: false,
        body: serde_json::to_value(body).unwrap_or(Value::Null),
    }
}

fn err_unauthenticated(msg: &'static str) -> ApiFailure {
    ApiFailure::new(ApiErrorCode::Unauthenticated, msg, "req_admin", None)
}

fn err_forbidden(msg: &'static str) -> ApiFailure {
    ApiFailure::new(ApiErrorCode::Forbidden, msg, "req_admin", None)
}

fn err_not_found(msg: &'static str) -> ApiFailure {
    ApiFailure::new(ApiErrorCode::NotFound, msg, "req_admin", None)
}

fn err_validation(msg: &'static str) -> ApiFailure {
    ApiFailure::new(ApiErrorCode::ValidationFailed, msg, "req_admin", None)
}

fn err_internal(msg: &'static str) -> ApiFailure {
    ApiFailure::new(ApiErrorCode::Internal, msg, "req_admin", None)
}

/// Authenticate an administrative request using Bearer token authorization.
pub async fn authenticate_admin_request(
    pool: &PgPool,
    authorization_header: Option<&str>,
) -> AdminAuthContext {
    let raw_header = match authorization_header {
        Some(h) if h.starts_with("Bearer ") => h.trim_start_matches("Bearer ").trim(),
        _ => return AdminAuthContext::Unauthenticated,
    };

    // User access token presented to an admin route returns forbidden (contracts.md §9.1)
    if is_user_token(raw_header) {
        return AdminAuthContext::UserTokenForbidden;
    }

    let token_hash = hash_admin_token(raw_header);
    match get_admin_session_by_token_hash(pool, &token_hash).await {
        Ok(Some(session)) => {
            let admin_id = session.admin_id;
            AdminAuthContext::Authenticated { session, admin_id }
        }
        _ => AdminAuthContext::Unauthenticated,
    }
}

/// 9.3 `POST /admin/v1/auth/challenge`
///
/// Issue a fresh cryptographic WebAuthn challenge for passkey assertion.
pub async fn post_admin_auth_challenge(
    pool: &PgPool,
    config: &AdminConfig,
    body: &AdminChallengeRequest,
    now: DateTime<Utc>,
) -> Result<ApiResponse<Value>, ApiFailure> {
    let _admin = match get_admin_by_username(pool, &body.username).await {
        Ok(Some(a)) => a,
        _ => return Err(err_not_found("Administrator account does not exist.")),
    };

    let challenge = WebAuthnChallenge::new(&config.rp_id, now, config.challenge_ttl_secs);
    Ok(ok_response(challenge))
}

/// 9.4 `POST /admin/v1/auth/login`
///
/// Authenticate administrator using WebAuthn passkey assertion.
pub async fn post_admin_auth_login(
    pool: &PgPool,
    config: &AdminConfig,
    active_challenge: &WebAuthnChallenge,
    body: &AdminLoginRequest,
    now: DateTime<Utc>,
) -> Result<ApiResponse<Value>, ApiFailure> {
    let admin = match get_admin_by_username(pool, &body.username).await {
        Ok(Some(a)) => a,
        _ => return Err(err_not_found("Administrator account does not exist.")),
    };

    let verification = verify_webauthn_assertion(
        &body.assertion,
        active_challenge,
        &admin.passkey_credential_id,
        &admin.passkey_public_key,
        admin.passkey_counter,
        &config.origin,
        now,
    );

    let new_counter = match verification {
        WebAuthnVerificationResult::Success { new_counter } => new_counter,
        WebAuthnVerificationResult::ChallengeMismatchOrExpired => {
            return Err(err_unauthenticated(
                "WebAuthn challenge has expired or does not match.",
            ))
        }
        WebAuthnVerificationResult::CounterRollbackDetected => {
            return Err(err_forbidden(
                "Passkey counter rollback detected. Possible cloned authenticator.",
            ))
        }
        WebAuthnVerificationResult::UserPresenceOrVerificationMissing => {
            return Err(err_forbidden(
                "User presence and user verification flags are required.",
            ))
        }
        WebAuthnVerificationResult::SignatureInvalid
        | WebAuthnVerificationResult::InvalidClientData => {
            return Err(err_unauthenticated(
                "WebAuthn assertion signature validation failed.",
            ))
        }
    };

    // Update monotonic counter
    if update_passkey_counter(pool, admin.id, new_counter)
        .await
        .is_err()
    {
        return Err(err_internal("failed to update passkey counter"));
    }

    // Issue short-lived session
    let token = generate_admin_token();
    let token_hash = hash_admin_token(&token);
    let expires_at = now + Duration::seconds(config.session_ttl_secs);

    let _session = match create_admin_session(pool, admin.id, &token_hash, expires_at).await {
        Ok(s) => s,
        Err(_) => return Err(err_internal("failed to create admin session")),
    };

    // Audit event
    let empty_meta = AuditMetadata::empty();
    let _ = append_audit_event(
        pool,
        &NewAuditEvent {
            actor_type: ActorType::Admin,
            actor_id: Some(admin.id),
            action: AuditAction::AdminLogin.as_str(),
            subject_type: "admin",
            subject_id: Some(admin.id),
            request_id: None,
            metadata: &empty_meta,
        },
    )
    .await;

    Ok(ok_response(serde_json::json!({
        "access_token": token,
        "token_type": "Bearer",
        "expires_in": config.session_ttl_secs
    })))
}

/// 9.5 `POST /admin/v1/auth/step-up`
///
/// Perform step-up re-authentication required before destructive actions.
pub async fn post_admin_auth_step_up(
    pool: &PgPool,
    config: &AdminConfig,
    active_challenge: &WebAuthnChallenge,
    auth: AdminAuthContext,
    body: &AdminStepUpRequest,
    now: DateTime<Utc>,
) -> Result<ApiResponse<Value>, ApiFailure> {
    let (session, admin_id) = match auth {
        AdminAuthContext::Authenticated { session, admin_id } => (session, admin_id),
        AdminAuthContext::UserTokenForbidden => {
            return Err(err_forbidden(
                "User access tokens cannot be used to access administrative endpoints.",
            ))
        }
        AdminAuthContext::Unauthenticated => {
            return Err(err_unauthenticated(
                "Administrative authentication required.",
            ))
        }
    };

    let admin = match get_admin(pool, admin_id).await {
        Ok(Some(a)) => a,
        _ => return Err(err_not_found("Administrator account does not exist.")),
    };

    let verification = verify_webauthn_assertion(
        &body.assertion,
        active_challenge,
        &admin.passkey_credential_id,
        &admin.passkey_public_key,
        admin.passkey_counter,
        &config.origin,
        now,
    );

    let new_counter = match verification {
        WebAuthnVerificationResult::Success { new_counter } => new_counter,
        _ => {
            return Err(err_unauthenticated(
                "Step-up re-authentication verification failed.",
            ))
        }
    };

    let _ = update_passkey_counter(pool, admin.id, new_counter).await;

    let step_up_until = now + Duration::seconds(config.step_up_ttl_secs);
    if update_admin_session_step_up(pool, session.id, step_up_until)
        .await
        .is_err()
    {
        return Err(err_internal("failed to update step up status"));
    }

    Ok(ok_response(serde_json::json!({
        "step_up_until": step_up_until,
        "expires_in": config.step_up_ttl_secs
    })))
}

/// 9.6 `POST /admin/v1/auth/recovery`
///
/// Redeem single-use offline recovery code to regain admin access.
pub async fn post_admin_auth_recovery(
    pool: &PgPool,
    config: &AdminConfig,
    body: &AdminRecoveryRequest,
    now: DateTime<Utc>,
) -> Result<ApiResponse<Value>, ApiFailure> {
    let admin = match get_admin_by_username(pool, &body.username).await {
        Ok(Some(a)) => a,
        _ => return Err(err_not_found("Administrator account does not exist.")),
    };

    // Check if recovery code was already consumed
    if admin.recovery_used_at.is_some() {
        return Err(err_validation(
            "Offline recovery code has already been consumed.",
        ));
    }

    // Verify constant-time
    if !verify_recovery_code(&body.recovery_code, &admin.recovery_code_hash) {
        return Err(err_unauthenticated("Invalid recovery code."));
    }

    // Mark single-use consumption
    if consume_recovery_code(pool, admin.id).await.is_err() {
        return Err(err_internal("failed to consume recovery code"));
    }

    // Issue emergency session
    let token = generate_admin_token();
    let token_hash = hash_admin_token(&token);
    let expires_at = now + Duration::seconds(config.session_ttl_secs);

    let _session = match create_admin_session(pool, admin.id, &token_hash, expires_at).await {
        Ok(s) => s,
        Err(_) => return Err(err_internal("failed to create session")),
    };

    // Audit event without recovery code material (INV-5)
    let empty_meta = AuditMetadata::empty();
    let _ = append_audit_event(
        pool,
        &NewAuditEvent {
            actor_type: ActorType::Admin,
            actor_id: Some(admin.id),
            action: AuditAction::AdminRecoveryUsed.as_str(),
            subject_type: "admin",
            subject_id: Some(admin.id),
            request_id: None,
            metadata: &empty_meta,
        },
    )
    .await;

    Ok(ok_response(serde_json::json!({
        "access_token": token,
        "token_type": "Bearer",
        "expires_in": config.session_ttl_secs,
        "recovery_warning": "Recovery code used. Enroll a new passkey immediately."
    })))
}

/// 9.7 `POST /admin/v1/users/{id}/suspend`
///
/// Suspend a user account, terminate all running tunnels, and revoke sessions (`SEC-015`).
pub async fn post_admin_suspend_user(
    pool: &PgPool,
    auth: AdminAuthContext,
    user_id: Uuid,
    body: Option<&AdminSuspendRequest>,
    now: DateTime<Utc>,
) -> Result<ApiResponse<Value>, ApiFailure> {
    let (session, admin_id) = match auth {
        AdminAuthContext::Authenticated { session, admin_id } => (session, admin_id),
        AdminAuthContext::UserTokenForbidden => {
            return Err(err_forbidden(
                "User access tokens cannot be used to access administrative endpoints.",
            ))
        }
        AdminAuthContext::Unauthenticated => {
            return Err(err_unauthenticated(
                "Administrative authentication required.",
            ))
        }
    };

    // Destructive actions strictly require active step-up
    if !is_session_stepped_up(&session, now) {
        return Err(err_forbidden(
            "Destructive operation requires recent step-up re-authentication.",
        ));
    }

    let reason_str = body
        .and_then(|b| b.reason.as_deref())
        .unwrap_or("admin_suspended");
    let updated =
        match set_user_status(pool, user_id, UserStatus::Suspended, Some(reason_str)).await {
            Ok(count) => count,
            Err(_) => return Err(err_internal("failed to suspend user")),
        };

    if updated == 0 {
        return Err(err_not_found("User account not found."));
    }

    let metadata =
        AuditMetadata::with_reason(reason_str).unwrap_or_else(|_| AuditMetadata::empty());
    let _ = append_audit_event(
        pool,
        &NewAuditEvent {
            actor_type: ActorType::Admin,
            actor_id: Some(admin_id),
            action: AuditAction::AdminUserSuspended.as_str(),
            subject_type: "user",
            subject_id: Some(user_id),
            request_id: None,
            metadata: &metadata,
        },
    )
    .await;

    Ok(ok_response(serde_json::json!({
        "user_id": user_id,
        "status": "suspended"
    })))
}

/// 9.8 `POST /admin/v1/users/{id}/unsuspend`
///
/// Reinstate a previously suspended user account.
pub async fn post_admin_unsuspend_user(
    pool: &PgPool,
    auth: AdminAuthContext,
    user_id: Uuid,
    now: DateTime<Utc>,
) -> Result<ApiResponse<Value>, ApiFailure> {
    let (session, admin_id) = match auth {
        AdminAuthContext::Authenticated { session, admin_id } => (session, admin_id),
        AdminAuthContext::UserTokenForbidden => {
            return Err(err_forbidden(
                "User access tokens cannot be used to access administrative endpoints.",
            ))
        }
        AdminAuthContext::Unauthenticated => {
            return Err(err_unauthenticated(
                "Administrative authentication required.",
            ))
        }
    };

    if !is_session_stepped_up(&session, now) {
        return Err(err_forbidden(
            "Destructive operation requires recent step-up re-authentication.",
        ));
    }

    let updated = match set_user_status(pool, user_id, UserStatus::Active, None).await {
        Ok(count) => count,
        Err(_) => return Err(err_internal("failed to unsuspend user")),
    };

    if updated == 0 {
        return Err(err_not_found("User account not found."));
    }

    let empty_meta = AuditMetadata::empty();
    let _ = append_audit_event(
        pool,
        &NewAuditEvent {
            actor_type: ActorType::Admin,
            actor_id: Some(admin_id),
            action: AuditAction::AdminUserUnsuspended.as_str(),
            subject_type: "user",
            subject_id: Some(user_id),
            request_id: None,
            metadata: &empty_meta,
        },
    )
    .await;

    Ok(ok_response(serde_json::json!({
        "user_id": user_id,
        "status": "active"
    })))
}

/// 9.9 `POST /admin/v1/tunnels/{id}/terminate`
///
/// Terminate an active tunnel and release its allocations (`SEC-015`).
pub async fn post_admin_terminate_tunnel(
    pool: &PgPool,
    auth: AdminAuthContext,
    tunnel_id: Uuid,
    body: Option<&AdminTerminateRequest>,
    now: DateTime<Utc>,
) -> Result<ApiResponse<Value>, ApiFailure> {
    let (session, admin_id) = match auth {
        AdminAuthContext::Authenticated { session, admin_id } => (session, admin_id),
        AdminAuthContext::UserTokenForbidden => {
            return Err(err_forbidden(
                "User access tokens cannot be used to access administrative endpoints.",
            ))
        }
        AdminAuthContext::Unauthenticated => {
            return Err(err_unauthenticated(
                "Administrative authentication required.",
            ))
        }
    };

    if !is_session_stepped_up(&session, now) {
        return Err(err_forbidden(
            "Destructive operation requires recent step-up re-authentication.",
        ));
    }

    let reason_str = body
        .and_then(|b| b.reason.as_deref())
        .unwrap_or("operator_terminated");
    let terminated = match db_terminate_tunnel(pool, tunnel_id, reason_str).await {
        Ok(t) => t,
        Err(_) => return Err(err_internal("failed to terminate tunnel")),
    };

    if !terminated {
        return Err(err_not_found(
            "Tunnel not found or already in terminal state.",
        ));
    }

    let metadata =
        AuditMetadata::with_reason(reason_str).unwrap_or_else(|_| AuditMetadata::empty());
    let _ = append_audit_event(
        pool,
        &NewAuditEvent {
            actor_type: ActorType::Admin,
            actor_id: Some(admin_id),
            action: AuditAction::TunnelTerminated.as_str(),
            subject_type: "tunnel",
            subject_id: Some(tunnel_id),
            request_id: None,
            metadata: &metadata,
        },
    )
    .await;

    Ok(ok_response(serde_json::json!({
        "tunnel_id": tunnel_id,
        "status": "terminated"
    })))
}

/// 9.10 `POST /admin/v1/relays/{id}/drain`
///
/// Place a relay into draining state so the candidate selector skips it (`REL-002`).
pub async fn post_admin_drain_relay(
    pool: &PgPool,
    auth: AdminAuthContext,
    relay_id: Uuid,
    now: DateTime<Utc>,
) -> Result<ApiResponse<Value>, ApiFailure> {
    let (session, admin_id) = match auth {
        AdminAuthContext::Authenticated { session, admin_id } => (session, admin_id),
        AdminAuthContext::UserTokenForbidden => {
            return Err(err_forbidden(
                "User access tokens cannot be used to access administrative endpoints.",
            ))
        }
        AdminAuthContext::Unauthenticated => {
            return Err(err_unauthenticated(
                "Administrative authentication required.",
            ))
        }
    };

    if !is_session_stepped_up(&session, now) {
        return Err(err_forbidden(
            "Destructive operation requires recent step-up re-authentication.",
        ));
    }

    let updated = match set_relay_status(pool, relay_id, RelayStatus::Draining, false).await {
        Ok(u) => u,
        Err(_) => return Err(err_internal("failed to drain relay")),
    };

    if !updated {
        return Err(err_not_found("Relay node not found."));
    }

    let empty_meta = AuditMetadata::empty();
    let _ = append_audit_event(
        pool,
        &NewAuditEvent {
            actor_type: ActorType::Admin,
            actor_id: Some(admin_id),
            action: AuditAction::AdminRelayDrained.as_str(),
            subject_type: "relay",
            subject_id: Some(relay_id),
            request_id: None,
            metadata: &empty_meta,
        },
    )
    .await;

    Ok(ok_response(serde_json::json!({
        "relay_id": relay_id,
        "status": "draining"
    })))
}

/// 9.11 `GET /admin/v1/audit-events`
///
/// View audit events with pagination and data minimization.
pub async fn get_admin_audit_events(
    pool: &PgPool,
    auth: AdminAuthContext,
    cursor: Option<i64>,
    limit: Option<i64>,
) -> Result<ApiResponse<Value>, ApiFailure> {
    match auth {
        AdminAuthContext::Authenticated { .. } => {}
        AdminAuthContext::UserTokenForbidden => {
            return Err(err_forbidden(
                "User access tokens cannot be used to access administrative endpoints.",
            ))
        }
        AdminAuthContext::Unauthenticated => {
            return Err(err_unauthenticated(
                "Administrative authentication required.",
            ))
        }
    }

    let limit = limit.unwrap_or(50).clamp(1, 100);
    let after_id = cursor.unwrap_or(0);

    let rows = match sqlx::query_as::<_, crate::db::entities::AuditEvent>(
        "select id, occurred_at, actor_type, actor_id, action, subject_type, subject_id, request_id, metadata
         from audit_events
         where id > $1
         order by id asc
         limit $2",
    )
    .bind(after_id)
    .bind(limit)
    .fetch_all(pool)
    .await {
        Ok(r) => r,
        Err(_) => return Err(err_internal("failed to fetch audit events")),
    };

    let next_cursor = rows.last().map(|r| r.id);
    let serialized_events: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "occurred_at": r.occurred_at,
                "actor_type": r.actor_type,
                "actor_id": r.actor_id,
                "action": r.action,
                "subject_type": r.subject_type,
                "subject_id": r.subject_id,
                "request_id": r.request_id,
                "metadata": r.metadata,
            })
        })
        .collect();

    Ok(ok_response(serde_json::json!({
        "events": serialized_events,
        "next_cursor": next_cursor
    })))
}
