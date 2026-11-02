//! Invitation, guest claim, admission, and usage routes (`T-044`, `FR-007`, `FR-008`, `FR-009`,
//! `SEC-007`, `SEC-009`, `PRIV-002`, `contracts.md` §§5.11–5.14, §6).
//!
//! ## Security invariants enforced here
//! 1. **High-entropy, one-time invitation secrets (`SEC-007`, `FR-007`):**
//!    [`generate_invitation_secret`] draws 192 bits (24 bytes) from two OS CSPRNG UUIDv4 values
//!    and encodes them as `"gtt_inv_"` + 32 base64url characters (`40` chars total, matching
//!    `^[A-Za-z0-9_-]{22,64}$`). Only [`hash_invitation_secret`] (`SHA-256`) is stored in
//!    PostgreSQL (`invitations.token_hash`). The plaintext secret is returned once in
//!    `CreateInvitationResponse` and is never stored in `invitations`, `idempotency_keys`, or
//!    `audit_events`.
//! 2. **Uniform `invitation_invalid` refusal (`FR-007`, `contracts.md` §6):**
//!    Unknown, expired, revoked, and claim-exhausted invitations all return HTTP `422`
//!    `invitation_invalid` with identical error text so an attacker cannot probe invitation state.
//! 3. **Observed guest IP only (`contracts.md` §6, `INV-1`):**
//!    `claim_invitation` takes the guest's `observed_source_ip` from the transport connection,
//!    never from the JSON request body.
//! 4. **Owner-only isolation & masked IPs (`SEC-009`, `PRIV-002`):**
//!    Cross-owner access returns `404 not_found`, never `403 forbidden`. Listing admissions masks
//!    guest source IPs (`a.b.c.0/24` or `a:b:c::/48`) via [`mask_ip_for_owner`].

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use gtt_protocol::http::{
    AllocatedPortDto, ApiErrorCode, ClaimInvitationRequest, ClaimInvitationResponse, ClaimRelayDto,
    CreateInvitationRequest, CreateInvitationResponse, ErrorDetail, Protocol as WireProtocol,
};
use gtt_protocol::validate::{is_valid_uuid, scan_json_for_local_target, ValidationError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::audit::{AuditAction, AuditMetadata};
use crate::db::entities::ActorType;
use crate::db::ops::{append_audit_event, usage_since, NewAuditEvent};
use crate::db::relay::{get_live_allocation, get_relay_node, list_allocation_ports};
use crate::db::sharing::{
    admission_tunnel_if_owned, admit_guest, assert_valid_source_ip,
    claim_invitation as db_claim_invitation, create_invitation as db_create_invitation,
    invitation_tunnel_if_owned, list_invitations as db_list_invitations, list_live_admissions,
    revoke_admission as db_revoke_admission, revoke_invitation as db_revoke_invitation,
};
use crate::db::tunnels::{get_tunnel as db_get_tunnel, list_mappings};
use crate::limiter::enforce_quota;
use crate::policy::{decide, Actor, Decision, ErrorCode as PolicyErrorCode, Operation, Target};
use crate::redaction::mask_ip_for_owner;
use crate::routes::tunnels::{ApiFailure, ApiResponse, MAX_REQUEST_BODY_BYTES};

/// Prefix for generated invitation secrets so secret-leak scanners can deterministically flag
/// any accidental log or diagnostic leak (`SEC-007`, `PRIV-002`).
pub const INVITATION_SECRET_PREFIX: &str = "gtt_inv_";

/// Number of CSPRNG bytes encoded into each invitation secret (`24` bytes = `192` bits `>= 128`
/// bits required by `SEC-007`).
pub const INVITATION_ENTROPY_BYTES: usize = 24;

/// Configuration for invitation and admission routes (`T-044`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvitationRouteConfig {
    /// Maximum unrevoked, non-expired invitations per tunnel (`SEC-008`).
    pub max_live_invitations_per_tunnel: i64,
    /// Default admission lifetime cap in seconds (`contracts.md` §6).
    pub max_admission_ttl_seconds: i64,
}

impl Default for InvitationRouteConfig {
    fn default() -> Self {
        Self {
            max_live_invitations_per_tunnel: 20,
            max_admission_ttl_seconds: 86_400,
        }
    }
}

/// Summary of an invitation returned by `GET /v1/tunnels/{id}/invitations` (`contracts.md` §5.12,
/// `FR-007`).
///
/// Deliberately has **no `secret` or `token_hash` field**: secrets are shown once at creation and
/// never returned again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvitationSummaryDto {
    /// Invitation UUID.
    pub id: String,
    /// Owning tunnel UUID.
    pub tunnel_id: String,
    /// Maximum claims allowed (`1..=50`).
    pub max_claims: u8,
    /// Claims used so far (`0..=max_claims`).
    pub claim_count: u8,
    /// Creation timestamp (RFC 3339 UTC).
    pub created_at: String,
    /// Expiration timestamp (RFC 3339 UTC).
    pub expires_at: String,
    /// Revocation timestamp (RFC 3339 UTC), if revoked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
}

/// Summary of a live guest admission returned by `GET /v1/tunnels/{id}/admissions`
/// (`contracts.md` §5.13, `PRIV-002`).
///
/// Carries only the masked prefix (`a.b.c.0/24` or `a:b:c::/48`), never the raw guest IP.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuestAdmissionSummaryDto {
    /// Admission UUID.
    pub id: String,
    /// Invitation UUID that granted this admission.
    pub invitation_id: String,
    /// Tunnel UUID.
    pub tunnel_id: String,
    /// Masked guest source IP (`a.b.c.0/24` or `a:b:c::/48`, `PRIV-002`).
    pub masked_source_ip: String,
    /// Creation timestamp (RFC 3339 UTC).
    pub created_at: String,
    /// Expiration timestamp (RFC 3339 UTC).
    pub expires_at: String,
}

/// Aggregated usage summary returned by `GET /v1/tunnels/{id}/usage` (`contracts.md` §5.14).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TunnelUsageSummaryDto {
    /// Tunnel UUID.
    pub tunnel_id: String,
    /// Window start timestamp (RFC 3339 UTC).
    pub since: String,
    /// Total bytes transferred (`bytes_from_guests + bytes_to_guests`) since `since`.
    pub total_bytes: i64,
}

/// Generate a fresh invitation secret with 192 bits of OS CSPRNG entropy (`SEC-007`).
///
/// Format: `"gtt_inv_"` + 32 base64url characters (`40` chars total, matching
/// `^[A-Za-z0-9_-]{22,64}$` in `contracts.md` §15).
pub fn generate_invitation_secret() -> String {
    let u1 = Uuid::new_v4();
    let u2 = Uuid::new_v4();
    let mut raw = [0u8; INVITATION_ENTROPY_BYTES];
    for (idx, slot) in raw.iter_mut().enumerate() {
        if let Some(&b) = u1.as_bytes().get(idx) {
            *slot = b;
        } else if let Some(&b) = u2.as_bytes().get(idx.saturating_sub(16)) {
            *slot = b;
        }
    }
    let encoded = base64url_encode_24(&raw);
    format!("{INVITATION_SECRET_PREFIX}{encoded}")
}

/// Compute the domain-separated SHA-256 hash stored in `invitations.token_hash` (`SEC-007`).
pub fn hash_invitation_secret(secret: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"gtt-invitation-secret-v1\0");
    hasher.update(secret.as_bytes());
    hasher.finalize().into()
}

const BASE64URL_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn base64url_encode_24(bytes: &[u8; 24]) -> String {
    let mut out = String::with_capacity(32);
    let mut chunk_idx = 0usize;
    while chunk_idx < 24 {
        let b0 = bytes.get(chunk_idx).copied().unwrap_or(0);
        let b1 = bytes.get(chunk_idx + 1).copied().unwrap_or(0);
        let b2 = bytes.get(chunk_idx + 2).copied().unwrap_or(0);

        let i0 = usize::from(b0 >> 2);
        let i1 = usize::from(((b0 & 0x03) << 4) | (b1 >> 4));
        let i2 = usize::from(((b1 & 0x0f) << 2) | (b2 >> 6));
        let i3 = usize::from(b2 & 0x3f);

        for idx in [i0, i1, i2, i3] {
            if let Some(&ch) = BASE64URL_ALPHABET.get(idx) {
                out.push(char::from(ch));
            }
        }
        chunk_idx += 3;
    }
    out
}

/// Parse and validate a `POST /v1/tunnels/{id}/invitations` body (`contracts.md` §§5.11, 15).
pub fn parse_create_invitation_request(
    raw_body: &str,
    request_id: &str,
) -> Result<CreateInvitationRequest, ApiFailure> {
    if raw_body.len() > MAX_REQUEST_BODY_BYTES {
        return Err(ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Request body exceeds maximum size of 16384 bytes.",
            request_id,
            None,
        ));
    }

    let raw_json: Value = serde_json::from_str(raw_body).map_err(|_| {
        ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Request body is not valid JSON.",
            request_id,
            None,
        )
    })?;

    if let Err(err) = scan_json_for_local_target(&raw_json) {
        return Err(validation_error_to_failure(&err, request_id));
    }

    let req: CreateInvitationRequest = serde_json::from_value(raw_json).map_err(|_| {
        ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Invalid create-invitation request schema.",
            request_id,
            None,
        )
    })?;

    if let Err(err) = req.validate() {
        return Err(validation_error_to_failure(&err, request_id));
    }

    Ok(req)
}

/// Parse and validate a `POST /v1/invitations/claim` body (`contracts.md` §§6, 15).
pub fn parse_claim_invitation_request(
    raw_body: &str,
    request_id: &str,
) -> Result<ClaimInvitationRequest, ApiFailure> {
    if raw_body.len() > MAX_REQUEST_BODY_BYTES {
        return Err(ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Request body exceeds maximum size of 16384 bytes.",
            request_id,
            None,
        ));
    }

    let raw_json: Value = serde_json::from_str(raw_body).map_err(|_| {
        ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Request body is not valid JSON.",
            request_id,
            None,
        )
    })?;

    if let Err(err) = scan_json_for_local_target(&raw_json) {
        return Err(validation_error_to_failure(&err, request_id));
    }

    let req: ClaimInvitationRequest = serde_json::from_value(raw_json).map_err(|_| {
        ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Invalid claim-invitation request schema.",
            request_id,
            None,
        )
    })?;

    if let Err(err) = req.validate() {
        return Err(validation_error_to_failure(&err, request_id));
    }

    Ok(req)
}

/// Route 12: `POST /v1/tunnels/{id}/invitations` (`contracts.md` §5.11, `FR-007`, `SEC-007`).
///
/// Generates a 192-bit CSPRNG secret, stores only its SHA-256 `token_hash` in PostgreSQL, writes
/// an `invitation.created` audit event, and returns `201 CreateInvitationResponse` with the
/// plaintext `secret` shown once.
pub async fn create_invitation(
    pool: &PgPool,
    config: &InvitationRouteConfig,
    actor: Actor,
    tunnel_id_str: &str,
    raw_body: &str,
    request_id: &str,
) -> Result<ApiResponse<CreateInvitationResponse>, ApiFailure> {
    let Ok(tunnel_id) = Uuid::parse_str(tunnel_id_str) else {
        return Err(ApiFailure::new(
            ApiErrorCode::NotFound,
            "Tunnel not found.",
            request_id,
            None,
        ));
    };

    let caller_id = require_user_owner_check(actor, Operation::CreateInvitation, request_id)?;

    let maybe_tunnel = db_get_tunnel(pool, caller_id, tunnel_id)
        .await
        .map_err(|_| internal_failure(request_id))?;
    let owner_ref = maybe_tunnel.as_ref().map(|t| &t.owner_id);
    if let Decision::Deny(code) = decide(
        actor,
        Operation::CreateInvitation,
        Target::UserOwned { owner: owner_ref },
    ) {
        return Err(policy_error_to_failure(code, request_id));
    }
    let Some(tunnel) = maybe_tunnel else {
        return Err(ApiFailure::new(
            ApiErrorCode::NotFound,
            "Tunnel not found.",
            request_id,
            None,
        ));
    };

    let req = parse_create_invitation_request(raw_body, request_id)?;

    if tunnel.state != "created" && tunnel.state != "active" {
        return Err(ApiFailure::new(
            ApiErrorCode::Conflict,
            "Tunnel is in a terminal state and cannot issue new invitations.",
            request_id,
            None,
        ));
    }

    // Check per-tunnel live invitation quota (`SEC-008`).
    let existing = db_list_invitations(pool, caller_id, tunnel.id)
        .await
        .map_err(|_| internal_failure(request_id))?;
    let now = Utc::now();
    let live_count = i64::try_from(
        existing
            .iter()
            .filter(|inv| inv.revoked_at.is_none() && inv.expires_at > now)
            .count(),
    )
    .unwrap_or(i64::MAX);
    enforce_quota(
        live_count,
        config.max_live_invitations_per_tunnel,
        "live invitations per tunnel",
        request_id,
    )?;

    let secret = generate_invitation_secret();
    let token_hash = hash_invitation_secret(&secret);
    let expires_at = now + Duration::seconds(i64::from(req.expires_in_seconds));

    let inv = db_create_invitation(
        pool,
        tunnel.id,
        caller_id,
        &token_hash,
        i32::from(req.max_claims),
        expires_at,
    )
    .await
    .map_err(|_| internal_failure(request_id))?;

    // Audit event (`SEC-013`, `T-046`): never includes `secret` or `token_hash` (`INV-5`).
    let metadata = AuditMetadata::default();
    let _ = append_audit_event(
        pool,
        &NewAuditEvent {
            actor_type: ActorType::User,
            actor_id: Some(caller_id),
            action: AuditAction::InvitationCreated.as_str(),
            subject_type: "invitation",
            subject_id: Some(inv.id),
            request_id: Some(request_id),
            metadata: &metadata,
        },
    )
    .await
    .map_err(|_| internal_failure(request_id))?;

    let claim_count = u8::try_from(inv.claim_count).unwrap_or(0);
    let dto = CreateInvitationResponse {
        id: inv.id.to_string(),
        tunnel_id: inv.tunnel_id.to_string(),
        secret,
        max_claims: req.max_claims,
        claim_count,
        created_at: inv.created_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        expires_at: inv.expires_at.to_rfc3339_opts(SecondsFormat::Secs, true),
    };
    dto.validate().map_err(|_| internal_failure(request_id))?;

    Ok(ApiResponse {
        status: 201,
        replayed: false,
        body: dto,
    })
}

/// Route 13: `GET /v1/tunnels/{id}/invitations` (`contracts.md` §5.12, `FR-007`, `SEC-007`).
///
/// Lists invitations for a tunnel owned by the caller. Never includes secrets or token hashes.
pub async fn list_invitations(
    pool: &PgPool,
    actor: Actor,
    tunnel_id_str: &str,
    request_id: &str,
) -> Result<ApiResponse<Vec<InvitationSummaryDto>>, ApiFailure> {
    let Ok(tunnel_id) = Uuid::parse_str(tunnel_id_str) else {
        return Err(ApiFailure::new(
            ApiErrorCode::NotFound,
            "Tunnel not found.",
            request_id,
            None,
        ));
    };

    let caller_id = require_user_owner_check(actor, Operation::ViewTunnel, request_id)?;

    let maybe_tunnel = db_get_tunnel(pool, caller_id, tunnel_id)
        .await
        .map_err(|_| internal_failure(request_id))?;
    let owner_ref = maybe_tunnel.as_ref().map(|t| &t.owner_id);
    if let Decision::Deny(code) = decide(
        actor,
        Operation::ViewTunnel,
        Target::UserOwned { owner: owner_ref },
    ) {
        return Err(policy_error_to_failure(code, request_id));
    }
    if maybe_tunnel.is_none() {
        return Err(ApiFailure::new(
            ApiErrorCode::NotFound,
            "Tunnel not found.",
            request_id,
            None,
        ));
    }

    let rows = db_list_invitations(pool, caller_id, tunnel_id)
        .await
        .map_err(|_| internal_failure(request_id))?;

    let mut out = Vec::with_capacity(rows.len());
    for inv in rows {
        let max_claims = u8::try_from(inv.max_claims).unwrap_or(50);
        let claim_count = u8::try_from(inv.claim_count).unwrap_or(0);
        out.push(InvitationSummaryDto {
            id: inv.id.to_string(),
            tunnel_id: inv.tunnel_id.to_string(),
            max_claims,
            claim_count,
            created_at: inv.created_at.to_rfc3339_opts(SecondsFormat::Secs, true),
            expires_at: inv.expires_at.to_rfc3339_opts(SecondsFormat::Secs, true),
            revoked_at: inv
                .revoked_at
                .map(|ts| ts.to_rfc3339_opts(SecondsFormat::Secs, true)),
        });
    }

    Ok(ApiResponse {
        status: 200,
        replayed: false,
        body: out,
    })
}

/// Route 14: `DELETE /v1/invitations/{id}` (`contracts.md` §5.12, `FR-009`, `SEC-009`).
///
/// Naturally idempotent (`contracts.md` §8): revokes the invitation and all guest admissions it
/// granted; returns `204` if owned by the caller, or `404 not_found` if absent or owned by
/// someone else.
pub async fn revoke_invitation(
    pool: &PgPool,
    actor: Actor,
    invitation_id_str: &str,
    request_id: &str,
) -> Result<ApiResponse<()>, ApiFailure> {
    let Ok(invitation_id) = Uuid::parse_str(invitation_id_str) else {
        return Err(ApiFailure::new(
            ApiErrorCode::NotFound,
            "Invitation not found.",
            request_id,
            None,
        ));
    };

    let caller_id = require_user_owner_check(actor, Operation::RevokeInvitation, request_id)?;

    let maybe_tunnel_id = invitation_tunnel_if_owned(pool, caller_id, invitation_id)
        .await
        .map_err(|_| internal_failure(request_id))?;
    let owner_opt = maybe_tunnel_id.map(|_| caller_id);
    if let Decision::Deny(code) = decide(
        actor,
        Operation::RevokeInvitation,
        Target::UserOwned {
            owner: owner_opt.as_ref(),
        },
    ) {
        return Err(policy_error_to_failure(code, request_id));
    }
    if maybe_tunnel_id.is_none() {
        return Err(ApiFailure::new(
            ApiErrorCode::NotFound,
            "Invitation not found.",
            request_id,
            None,
        ));
    }

    let newly_revoked = db_revoke_invitation(pool, caller_id, invitation_id)
        .await
        .map_err(|_| internal_failure(request_id))?;

    if newly_revoked {
        let metadata = AuditMetadata::default();
        let _ = append_audit_event(
            pool,
            &NewAuditEvent {
                actor_type: ActorType::User,
                actor_id: Some(caller_id),
                action: AuditAction::InvitationRevoked.as_str(),
                subject_type: "invitation",
                subject_id: Some(invitation_id),
                request_id: Some(request_id),
                metadata: &metadata,
            },
        )
        .await
        .map_err(|_| internal_failure(request_id))?;
    }

    Ok(ApiResponse {
        status: 204,
        replayed: !newly_revoked,
        body: (),
    })
}

/// Route 15: `GET /v1/tunnels/{id}/admissions` (`contracts.md` §5.13, `PRIV-002`, `SEC-009`).
///
/// Lists live guest admissions for a tunnel owned by the caller, masking each guest source IP to
/// `a.b.c.0/24` or `a:b:c::/48` (`mask_ip_for_owner`).
pub async fn list_admissions(
    pool: &PgPool,
    actor: Actor,
    tunnel_id_str: &str,
    request_id: &str,
) -> Result<ApiResponse<Vec<GuestAdmissionSummaryDto>>, ApiFailure> {
    let Ok(tunnel_id) = Uuid::parse_str(tunnel_id_str) else {
        return Err(ApiFailure::new(
            ApiErrorCode::NotFound,
            "Tunnel not found.",
            request_id,
            None,
        ));
    };

    let caller_id = require_user_owner_check(actor, Operation::ListAdmissions, request_id)?;

    let maybe_tunnel = db_get_tunnel(pool, caller_id, tunnel_id)
        .await
        .map_err(|_| internal_failure(request_id))?;
    let owner_ref = maybe_tunnel.as_ref().map(|t| &t.owner_id);
    if let Decision::Deny(code) = decide(
        actor,
        Operation::ListAdmissions,
        Target::UserOwned { owner: owner_ref },
    ) {
        return Err(policy_error_to_failure(code, request_id));
    }
    if maybe_tunnel.is_none() {
        return Err(ApiFailure::new(
            ApiErrorCode::NotFound,
            "Tunnel not found.",
            request_id,
            None,
        ));
    }

    let rows = list_live_admissions(pool, tunnel_id, Utc::now())
        .await
        .map_err(|_| internal_failure(request_id))?;

    let mut out = Vec::with_capacity(rows.len());
    for adm in rows {
        let masked_source_ip =
            mask_ip_for_owner(&adm.source_ip).unwrap_or_else(|| "<masked-ip>".to_owned());
        out.push(GuestAdmissionSummaryDto {
            id: adm.id.to_string(),
            invitation_id: adm.invitation_id.to_string(),
            tunnel_id: adm.tunnel_id.to_string(),
            masked_source_ip,
            created_at: adm.created_at.to_rfc3339_opts(SecondsFormat::Secs, true),
            expires_at: adm.expires_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        });
    }

    Ok(ApiResponse {
        status: 200,
        replayed: false,
        body: out,
    })
}

/// Route 16: `DELETE /v1/admissions/{id}` (`contracts.md` §5.13, `FR-009`, `SEC-009`).
///
/// Naturally idempotent: revokes a single guest admission if owned by the caller, or returns `404
/// not_found` if absent or owned by another user.
pub async fn revoke_admission(
    pool: &PgPool,
    actor: Actor,
    admission_id_str: &str,
    request_id: &str,
) -> Result<ApiResponse<()>, ApiFailure> {
    let Ok(admission_id) = Uuid::parse_str(admission_id_str) else {
        return Err(ApiFailure::new(
            ApiErrorCode::NotFound,
            "Admission not found.",
            request_id,
            None,
        ));
    };

    let caller_id = require_user_owner_check(actor, Operation::RevokeInvitation, request_id)?;

    let maybe_tunnel_id = admission_tunnel_if_owned(pool, caller_id, admission_id)
        .await
        .map_err(|_| internal_failure(request_id))?;
    let owner_opt = maybe_tunnel_id.map(|_| caller_id);
    if let Decision::Deny(code) = decide(
        actor,
        Operation::RevokeInvitation,
        Target::UserOwned {
            owner: owner_opt.as_ref(),
        },
    ) {
        return Err(policy_error_to_failure(code, request_id));
    }
    let Some(tunnel_id) = maybe_tunnel_id else {
        return Err(ApiFailure::new(
            ApiErrorCode::NotFound,
            "Admission not found.",
            request_id,
            None,
        ));
    };

    let newly_revoked = db_revoke_admission(pool, tunnel_id, admission_id)
        .await
        .map_err(|_| internal_failure(request_id))?;

    if newly_revoked {
        let metadata = AuditMetadata::default();
        let _ = append_audit_event(
            pool,
            &NewAuditEvent {
                actor_type: ActorType::User,
                actor_id: Some(caller_id),
                action: AuditAction::AdmissionRevoked.as_str(),
                subject_type: "guest_admission",
                subject_id: Some(admission_id),
                request_id: Some(request_id),
                metadata: &metadata,
            },
        )
        .await
        .map_err(|_| internal_failure(request_id))?;
    }

    Ok(ApiResponse {
        status: 204,
        replayed: !newly_revoked,
        body: (),
    })
}

/// Route 17: `GET /v1/tunnels/{id}/usage` (`contracts.md` §5.14, `SEC-009`).
pub async fn get_tunnel_usage(
    pool: &PgPool,
    actor: Actor,
    tunnel_id_str: &str,
    since: DateTime<Utc>,
    request_id: &str,
) -> Result<ApiResponse<TunnelUsageSummaryDto>, ApiFailure> {
    let Ok(tunnel_id) = Uuid::parse_str(tunnel_id_str) else {
        return Err(ApiFailure::new(
            ApiErrorCode::NotFound,
            "Tunnel not found.",
            request_id,
            None,
        ));
    };

    let caller_id = require_user_owner_check(actor, Operation::ViewTunnel, request_id)?;

    let maybe_tunnel = db_get_tunnel(pool, caller_id, tunnel_id)
        .await
        .map_err(|_| internal_failure(request_id))?;
    let owner_ref = maybe_tunnel.as_ref().map(|t| &t.owner_id);
    if let Decision::Deny(code) = decide(
        actor,
        Operation::ViewTunnel,
        Target::UserOwned { owner: owner_ref },
    ) {
        return Err(policy_error_to_failure(code, request_id));
    }
    if maybe_tunnel.is_none() {
        return Err(ApiFailure::new(
            ApiErrorCode::NotFound,
            "Tunnel not found.",
            request_id,
            None,
        ));
    }

    let total_bytes = usage_since(pool, tunnel_id, since)
        .await
        .map_err(|_| internal_failure(request_id))?;

    Ok(ApiResponse {
        status: 200,
        replayed: false,
        body: TunnelUsageSummaryDto {
            tunnel_id: tunnel_id.to_string(),
            since: since.to_rfc3339_opts(SecondsFormat::Secs, true),
            total_bytes,
        },
    })
}

/// Route 18: `POST /v1/invitations/claim` (`contracts.md` §6, `FR-007`, `FR-008`, `SEC-007`).
///
/// Unauthenticated guest route. The admitted IP is `observed_source_ip` (from the server-observed
/// connection, never the request body). Unknown, expired, revoked, or exhausted invitations all
/// return HTTP `422` `invitation_invalid`.
pub async fn claim_invitation(
    pool: &PgPool,
    config: &InvitationRouteConfig,
    raw_body: &str,
    observed_source_ip: &str,
    request_id: &str,
) -> Result<ApiResponse<ClaimInvitationResponse>, ApiFailure> {
    if let Decision::Deny(code) = decide(Actor::Anonymous, Operation::ClaimInvitation, Target::None)
    {
        return Err(policy_error_to_failure(code, request_id));
    }

    let req = parse_claim_invitation_request(raw_body, request_id)?;

    let clean_ip = observed_source_ip.trim();
    if assert_valid_source_ip(clean_ip).is_err() {
        return Err(ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Observed connection source IP is not a valid host address.",
            request_id,
            None,
        ));
    }

    let now = Utc::now();
    let token_hash = hash_invitation_secret(&req.secret);

    let maybe_inv = db_claim_invitation(pool, &token_hash, now)
        .await
        .map_err(|_| internal_failure(request_id))?;

    let Some(inv) = maybe_inv else {
        return Err(invitation_invalid_failure(request_id));
    };

    // Look up the tunnel and its live relay allocation.
    let maybe_tunnel = db_get_tunnel(pool, inv.created_by, inv.tunnel_id)
        .await
        .map_err(|_| internal_failure(request_id))?;
    let Some(tunnel) = maybe_tunnel else {
        return Err(invitation_invalid_failure(request_id));
    };
    if tunnel.state != "active" {
        return Err(invitation_invalid_failure(request_id));
    };

    let maybe_alloc = get_live_allocation(pool, tunnel.id)
        .await
        .map_err(|_| internal_failure(request_id))?;
    let Some(alloc) = maybe_alloc else {
        return Err(invitation_invalid_failure(request_id));
    };
    if alloc.expires_at <= now {
        return Err(invitation_invalid_failure(request_id));
    }

    let maybe_relay = get_relay_node(pool, alloc.relay_id)
        .await
        .map_err(|_| internal_failure(request_id))?;
    let Some(relay_node) = maybe_relay else {
        return Err(invitation_invalid_failure(request_id));
    };

    let alloc_ports = list_allocation_ports(pool, alloc.id)
        .await
        .map_err(|_| internal_failure(request_id))?;
    let mappings = list_mappings(pool, inv.created_by, tunnel.id)
        .await
        .map_err(|_| internal_failure(request_id))?;

    let mut ports_dto = Vec::with_capacity(alloc_ports.len());
    for ap in &alloc_ports {
        if let Some(m) = mappings.iter().find(|m| m.id == ap.mapping_id) {
            let Ok(pub_port) = u16::try_from(ap.public_port) else {
                continue;
            };
            let proto = match ap.protocol.as_str() {
                "tcp" => WireProtocol::Tcp,
                "udp" => WireProtocol::Udp,
                _ => continue,
            };
            ports_dto.push(AllocatedPortDto {
                mapping_key: m.mapping_key.clone(),
                protocol: proto,
                public_port: pub_port,
            });
        }
    }

    let max_ttl = now + Duration::seconds(config.max_admission_ttl_seconds.max(60));
    let admitted_until = inv.expires_at.min(alloc.expires_at).min(max_ttl);

    let admission = admit_guest(pool, inv.id, tunnel.id, clean_ip, admitted_until)
        .await
        .map_err(|_| internal_failure(request_id))?;

    let metadata = AuditMetadata::default();
    let _ = append_audit_event(
        pool,
        &NewAuditEvent {
            actor_type: ActorType::System,
            actor_id: None,
            action: AuditAction::InvitationClaimed.as_str(),
            subject_type: "guest_admission",
            subject_id: Some(admission.id),
            request_id: Some(request_id),
            metadata: &metadata,
        },
    )
    .await
    .map_err(|_| internal_failure(request_id))?;

    let resp = ClaimInvitationResponse {
        tunnel_id: tunnel.id.to_string(),
        game_profile_id: tunnel.game_profile_id,
        relay: ClaimRelayDto {
            region: relay_node.region,
            host: relay_node.public_host,
            ports: ports_dto,
        },
        admitted_until: admission
            .expires_at
            .to_rfc3339_opts(SecondsFormat::Secs, true),
    };
    resp.validate().map_err(|_| internal_failure(request_id))?;

    Ok(ApiResponse {
        status: 200,
        replayed: false,
        body: resp,
    })
}

fn require_user_owner_check(
    actor: Actor,
    op: Operation,
    request_id: &str,
) -> Result<Uuid, ApiFailure> {
    match actor {
        Actor::User { id, .. } => Ok(id),
        other => {
            let decision = decide(other, op, Target::UserOwned { owner: None });
            let code = decision.error().unwrap_or(PolicyErrorCode::NotFound);
            Err(policy_error_to_failure(code, request_id))
        }
    }
}

fn invitation_invalid_failure(request_id: &str) -> ApiFailure {
    ApiFailure::new(
        ApiErrorCode::InvitationInvalid,
        "The invitation secret is unknown, expired, revoked, or has no remaining claims.",
        request_id,
        None,
    )
}

fn policy_error_to_failure(code: PolicyErrorCode, request_id: &str) -> ApiFailure {
    let (api_code, msg) = match code {
        PolicyErrorCode::Unauthenticated => {
            (ApiErrorCode::Unauthenticated, "Authentication is required.")
        }
        PolicyErrorCode::Forbidden => (
            ApiErrorCode::Forbidden,
            "You are not permitted to perform this operation.",
        ),
        PolicyErrorCode::NotFound => (ApiErrorCode::NotFound, "Resource not found."),
        PolicyErrorCode::ValidationFailed => {
            (ApiErrorCode::ValidationFailed, "Request validation failed.")
        }
        PolicyErrorCode::Conflict => (
            ApiErrorCode::Conflict,
            "Operation conflicts with current resource state.",
        ),
    };
    ApiFailure::new(api_code, msg, request_id, None)
}

fn validation_error_to_failure(err: &ValidationError, request_id: &str) -> ApiFailure {
    match err {
        ValidationError::LocalTargetField(field) => ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Local destination fields are never accepted by the control plane (INV-1).",
            request_id,
            Some(vec![ErrorDetail {
                field: field.clone(),
                issue: "host-local target fields are forbidden (INV-1)".to_owned(),
            }]),
        ),
        ValidationError::InvalidField { field, issue } => ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Request body is invalid.",
            request_id,
            Some(vec![ErrorDetail {
                field: (*field).to_owned(),
                issue: (*issue).to_owned(),
            }]),
        ),
        ValidationError::JsonTooDeep => ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Request JSON nesting exceeds maximum depth.",
            request_id,
            None,
        ),
    }
}

fn internal_failure(request_id: &str) -> ApiFailure {
    let _ = is_valid_uuid;
    ApiFailure::new(
        ApiErrorCode::Internal,
        "An internal database error occurred.",
        request_id,
        None,
    )
}
