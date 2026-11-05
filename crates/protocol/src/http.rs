//! HTTP API request and response models (`contracts.md` §§3, 5, 6, 15 and `data-model.md` §7).
//!
//! Every request struct uses `#[serde(deny_unknown_fields)]` so unknown fields and `INV-1`
//! local-target fields are rejected at deserialization time. Every type also provides a
//! `.validate()` method enforcing the cross-field and format rules from `contracts.md` §15.

use serde::{Deserialize, Serialize};

use crate::validate::{
    is_valid_identifier, is_valid_invitation_secret, is_valid_pkce_verifier, is_valid_redirect_uri,
    is_valid_rfc3339_utc, is_valid_tunnel_name, is_valid_uuid, ValidationError,
};

/// Maximum mappings per tunnel (`contracts.md` §15: 1–8 entries).
pub const MAX_TUNNEL_MAPPINGS: usize = 8;

/// Wire protocol for a port mapping (`contracts.md` §15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    /// TCP stream forwarding.
    Tcp,
    /// UDP datagram forwarding.
    Udp,
}

impl Protocol {
    /// Wire representation (`"tcp"` or `"udp"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
        }
    }
}

/// Error codes enumerated in `contracts.md` §3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiErrorCode {
    /// Missing, invalid, or expired credentials (HTTP 401).
    Unauthenticated,
    /// Authenticated but not allowed (HTTP 403).
    Forbidden,
    /// Resource absent or not visible to the caller (HTTP 404).
    NotFound,
    /// Schema or domain validation failed (HTTP 400).
    ValidationFailed,
    /// State conflict (HTTP 409).
    Conflict,
    /// Same `Idempotency-Key` with a different request body (HTTP 422).
    IdempotencyMismatch,
    /// Rate limit exceeded (HTTP 429).
    RateLimited,
    /// Account quota reached (HTTP 403).
    QuotaExceeded,
    /// Expired, revoked, exhausted, or unknown invitation secret (HTTP 400).
    InvitationInvalid,
    /// No healthy relay with capacity (HTTP 503).
    RelayUnavailable,
    /// Client below minimum supported version (HTTP 426).
    ClientUpgradeRequired,
    /// Unexpected internal error (HTTP 500).
    Internal,
}

impl ApiErrorCode {
    /// The HTTP status code paired with this error code in `contracts.md` §3.
    pub fn http_status(self) -> u16 {
        match self {
            Self::Unauthenticated => 401,
            Self::Forbidden | Self::QuotaExceeded => 403,
            Self::NotFound => 404,
            Self::ValidationFailed | Self::InvitationInvalid => 400,
            Self::Conflict => 409,
            Self::IdempotencyMismatch => 422,
            Self::RateLimited => 429,
            Self::RelayUnavailable => 503,
            Self::ClientUpgradeRequired => 426,
            Self::Internal => 500,
        }
    }

    /// Lowercase `snake_case` wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unauthenticated => "unauthenticated",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not_found",
            Self::ValidationFailed => "validation_failed",
            Self::Conflict => "conflict",
            Self::IdempotencyMismatch => "idempotency_mismatch",
            Self::RateLimited => "rate_limited",
            Self::QuotaExceeded => "quota_exceeded",
            Self::InvitationInvalid => "invitation_invalid",
            Self::RelayUnavailable => "relay_unavailable",
            Self::ClientUpgradeRequired => "client_upgrade_required",
            Self::Internal => "internal",
        }
    }
}

/// One field-level error detail inside [`ErrorBody`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorDetail {
    /// Field name or path.
    pub field: String,
    /// Description of the validation issue.
    pub issue: String,
}

/// Inner object of [`ErrorResponse`] (`contracts.md` §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorBody {
    /// Standardized error code.
    pub code: ApiErrorCode,
    /// Human-readable summary that never echoes secrets or internal identifiers.
    pub message: String,
    /// Request correlation identifier.
    pub request_id: String,
    /// Optional per-field validation failures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Vec<ErrorDetail>>,
}

/// Top-level error envelope (`contracts/error.schema.json`, `contracts.md` §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorResponse {
    /// Error payload.
    pub error: ErrorBody,
}

impl ErrorResponse {
    /// Validate field bounds on an [`ErrorResponse`].
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.error.message.trim().is_empty() || self.error.message.len() > 512 {
            return Err(ValidationError::InvalidField {
                field: "error.message",
                issue: "must be 1-512 characters",
            });
        }
        if self.error.request_id.trim().is_empty() || self.error.request_id.len() > 128 {
            return Err(ValidationError::InvalidField {
                field: "error.request_id",
                issue: "must be 1-128 characters",
            });
        }
        if let Some(details) = &self.error.details {
            if details.len() > 32 {
                return Err(ValidationError::InvalidField {
                    field: "error.details",
                    issue: "at most 32 detail items are allowed",
                });
            }
            for item in details {
                if item.field.trim().is_empty() || item.field.len() > 64 {
                    return Err(ValidationError::InvalidField {
                        field: "error.details.field",
                        issue: "must be 1-64 characters",
                    });
                }
                if item.issue.trim().is_empty() || item.issue.len() > 256 {
                    return Err(ValidationError::InvalidField {
                        field: "error.details.issue",
                        issue: "must be 1-256 characters",
                    });
                }
            }
        }
        Ok(())
    }
}

/// One requested mapping in [`CreateTunnelRequest`] (`contracts.md` §5.6, `data-model.md` §7).
///
/// Contains **no** local host or port (`INV-1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MappingRequest {
    /// Profile-defined key (`^[a-z][a-z0-9-]{0,31}$`).
    pub mapping_key: String,
    /// Requested transport protocol.
    pub protocol: Protocol,
    /// Whether this mapping is enabled.
    pub enabled: bool,
}

/// Request body for `POST /v1/tunnels` (`contracts/create-tunnel-request.schema.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateTunnelRequest {
    /// User-facing tunnel name (1–64 chars after trimming, no control characters).
    pub name: String,
    /// Identifier of the game profile.
    pub game_profile_id: String,
    /// Requested port mappings (1–8 entries, unique keys).
    pub mappings: Vec<MappingRequest>,
}

impl CreateTunnelRequest {
    /// Validate `contracts.md` §15 rules on [`CreateTunnelRequest`].
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !is_valid_tunnel_name(&self.name) {
            return Err(ValidationError::InvalidField {
                field: "name",
                issue: "must be 1-64 characters after trimming with no control characters",
            });
        }
        if !is_valid_identifier(&self.game_profile_id) {
            return Err(ValidationError::InvalidField {
                field: "game_profile_id",
                issue: "must match ^[a-z][a-z0-9-]{0,31}$",
            });
        }
        if !(1..=MAX_TUNNEL_MAPPINGS).contains(&self.mappings.len()) {
            return Err(ValidationError::InvalidField {
                field: "mappings",
                issue: "must contain 1 to 8 mappings",
            });
        }
        for (idx, mapping) in self.mappings.iter().enumerate() {
            if !is_valid_identifier(&mapping.mapping_key) {
                return Err(ValidationError::InvalidField {
                    field: "mappings.mapping_key",
                    issue: "must match ^[a-z][a-z0-9-]{0,31}$",
                });
            }
            if let Some(earlier) = self.mappings.get(..idx) {
                if earlier.iter().any(|m| m.mapping_key == mapping.mapping_key) {
                    return Err(ValidationError::InvalidField {
                        field: "mappings.mapping_key",
                        issue: "mapping_key must be unique within a tunnel",
                    });
                }
            }
        }
        Ok(())
    }
}

/// Lifecycle state of a tunnel record (`contracts.md` §12, `data-model.md` §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TunnelState {
    /// Created, no relay allocated yet.
    Created,
    /// Active lifecycle intent.
    Active,
    /// Stopped by the owner (terminal).
    Stopped,
    /// Terminated by operator or abuse policy (terminal).
    Terminated,
    /// Reached expiry (terminal).
    Expired,
}

impl TunnelState {
    /// True if this state is terminal (`INV-7`).
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Stopped | Self::Terminated | Self::Expired)
    }
}

/// One mapping inside a [`Tunnel`] response (`contracts/tunnel.schema.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TunnelMappingDto {
    /// Profile-defined key.
    pub mapping_key: String,
    /// Transport protocol.
    pub protocol: Protocol,
    /// Zero-based wire index (`0..=7`).
    pub position: u8,
    /// Whether the mapping is enabled.
    pub enabled: bool,
}

/// Tunnel resource representation (`contracts/tunnel.schema.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tunnel {
    /// Tunnel UUID.
    pub id: String,
    /// User-facing name.
    pub name: String,
    /// Game profile identifier.
    pub game_profile_id: String,
    /// Profile revision captured at tunnel creation.
    pub profile_version: i32,
    /// Tunnel lifecycle state.
    pub state: TunnelState,
    /// Mappings in wire order.
    pub mappings: Vec<TunnelMappingDto>,
    /// Creation timestamp (RFC 3339 UTC).
    pub created_at: String,
    /// Start timestamp (RFC 3339 UTC), if started.
    pub started_at: Option<String>,
    /// Stop timestamp (RFC 3339 UTC); non-null iff `state` is terminal (`INV-7`).
    pub stopped_at: Option<String>,
    /// Expiry timestamp (RFC 3339 UTC).
    pub expires_at: String,
    /// Optional termination reason.
    pub termination_reason: Option<String>,
}

impl Tunnel {
    /// Validate [`Tunnel`] fields and `INV-7` terminal-state consistency.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !is_valid_uuid(&self.id) {
            return Err(ValidationError::InvalidField {
                field: "id",
                issue: "must be a valid UUID",
            });
        }
        if !is_valid_tunnel_name(&self.name) {
            return Err(ValidationError::InvalidField {
                field: "name",
                issue: "must be 1-64 characters after trimming with no control characters",
            });
        }
        if !is_valid_identifier(&self.game_profile_id) {
            return Err(ValidationError::InvalidField {
                field: "game_profile_id",
                issue: "must match ^[a-z][a-z0-9-]{0,31}$",
            });
        }
        if self.profile_version < 1 {
            return Err(ValidationError::InvalidField {
                field: "profile_version",
                issue: "must be >= 1",
            });
        }
        if !(1..=MAX_TUNNEL_MAPPINGS).contains(&self.mappings.len()) {
            return Err(ValidationError::InvalidField {
                field: "mappings",
                issue: "must contain 1 to 8 mappings",
            });
        }
        for (idx, m) in self.mappings.iter().enumerate() {
            if !is_valid_identifier(&m.mapping_key) {
                return Err(ValidationError::InvalidField {
                    field: "mappings.mapping_key",
                    issue: "must match ^[a-z][a-z0-9-]{0,31}$",
                });
            }
            if usize::from(m.position) != idx {
                return Err(ValidationError::InvalidField {
                    field: "mappings.position",
                    issue: "must match 0-based wire order",
                });
            }
            if let Some(earlier) = self.mappings.get(..idx) {
                if earlier.iter().any(|prev| prev.mapping_key == m.mapping_key) {
                    return Err(ValidationError::InvalidField {
                        field: "mappings.mapping_key",
                        issue: "mapping_key must be unique within a tunnel",
                    });
                }
            }
        }
        if !is_valid_rfc3339_utc(&self.created_at) || !is_valid_rfc3339_utc(&self.expires_at) {
            return Err(ValidationError::InvalidField {
                field: "created_at/expires_at",
                issue: "must be valid RFC 3339 UTC timestamps",
            });
        }
        if let Some(started) = &self.started_at {
            if !is_valid_rfc3339_utc(started) {
                return Err(ValidationError::InvalidField {
                    field: "started_at",
                    issue: "must be a valid RFC 3339 UTC timestamp",
                });
            }
        }
        if let Some(stopped) = &self.stopped_at {
            if !is_valid_rfc3339_utc(stopped) {
                return Err(ValidationError::InvalidField {
                    field: "stopped_at",
                    issue: "must be a valid RFC 3339 UTC timestamp",
                });
            }
        }
        // INV-7: terminal states must have stopped_at set; non-terminal states must not.
        if self.state.is_terminal() != self.stopped_at.is_some() {
            return Err(ValidationError::InvalidField {
                field: "stopped_at",
                issue: "stopped_at must be set if and only if state is terminal (INV-7)",
            });
        }
        if let Some(reason) = &self.termination_reason {
            if reason.is_empty() || reason.len() > 64 {
                return Err(ValidationError::InvalidField {
                    field: "termination_reason",
                    issue: "must be 1-64 characters when present",
                });
            }
        }
        Ok(())
    }
}

/// Relay connection details inside [`AllocationDto`] (`contracts.md` §5.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayEndpointDto {
    /// Relay region identifier.
    pub region: String,
    /// Relay public hostname.
    pub host: String,
    /// Relay QUIC tunnel port (`1..=65535`).
    pub tunnel_port: u16,
}

/// One allocated public port in [`AllocationDto`] or [`ClaimRelayDto`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocatedPortDto {
    /// Profile mapping key.
    pub mapping_key: String,
    /// Transport protocol.
    pub protocol: Protocol,
    /// Public port reserved on the relay (`1..=65535`).
    pub public_port: u16,
}

/// Relay allocation details inside [`StartTunnelResponse`] (`contracts.md` §5.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocationDto {
    /// Allocation UUID.
    pub id: String,
    /// Allocation expiry timestamp (RFC 3339 UTC).
    pub expires_at: String,
    /// Relay endpoint.
    pub relay: RelayEndpointDto,
    /// Allocated public ports.
    pub ports: Vec<AllocatedPortDto>,
}

/// Response body for `POST /v1/tunnels/{id}/start` (`contracts/start-tunnel-response.schema.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartTunnelResponse {
    /// Tunnel UUID.
    pub tunnel_id: String,
    /// Signed opaque allocation token.
    pub allocation_token: String,
    /// Allocation details.
    pub allocation: AllocationDto,
}

impl StartTunnelResponse {
    /// Validate [`StartTunnelResponse`] against `contracts.md` §5.8.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !is_valid_uuid(&self.tunnel_id) {
            return Err(ValidationError::InvalidField {
                field: "tunnel_id",
                issue: "must be a valid UUID",
            });
        }
        if self.allocation_token.trim().is_empty() || self.allocation_token.len() > 4096 {
            return Err(ValidationError::InvalidField {
                field: "allocation_token",
                issue: "must be 1-4096 characters",
            });
        }
        if !is_valid_uuid(&self.allocation.id) {
            return Err(ValidationError::InvalidField {
                field: "allocation.id",
                issue: "must be a valid UUID",
            });
        }
        if !is_valid_rfc3339_utc(&self.allocation.expires_at) {
            return Err(ValidationError::InvalidField {
                field: "allocation.expires_at",
                issue: "must be a valid RFC 3339 UTC timestamp",
            });
        }
        if self.allocation.relay.region.trim().is_empty() || self.allocation.relay.region.len() > 64
        {
            return Err(ValidationError::InvalidField {
                field: "allocation.relay.region",
                issue: "must be 1-64 characters",
            });
        }
        if self.allocation.relay.host.trim().is_empty() || self.allocation.relay.host.len() > 253 {
            return Err(ValidationError::InvalidField {
                field: "allocation.relay.host",
                issue: "must be 1-253 characters",
            });
        }
        if self.allocation.relay.tunnel_port == 0 {
            return Err(ValidationError::InvalidField {
                field: "allocation.relay.tunnel_port",
                issue: "must be 1..=65535",
            });
        }
        validate_allocated_ports(&self.allocation.ports)
    }
}

/// Response body for `POST /v1/tunnels/{id}/token` (`contracts.md` §5.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssueTunnelTokenResponse {
    /// Freshly signed opaque allocation token (`1..=4096` characters).
    pub allocation_token: String,
    /// Token expiration timestamp (RFC 3339 UTC).
    pub expires_at: String,
}

impl IssueTunnelTokenResponse {
    /// Validate [`IssueTunnelTokenResponse`] against `contracts.md` §5.9.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.allocation_token.trim().is_empty() || self.allocation_token.len() > 4096 {
            return Err(ValidationError::InvalidField {
                field: "allocation_token",
                issue: "must be 1-4096 characters",
            });
        }
        if !is_valid_rfc3339_utc(&self.expires_at) {
            return Err(ValidationError::InvalidField {
                field: "expires_at",
                issue: "must be a valid RFC 3339 UTC timestamp",
            });
        }
        Ok(())
    }
}

fn validate_allocated_ports(ports: &[AllocatedPortDto]) -> Result<(), ValidationError> {
    if !(1..=MAX_TUNNEL_MAPPINGS).contains(&ports.len()) {
        return Err(ValidationError::InvalidField {
            field: "ports",
            issue: "must contain 1 to 8 ports",
        });
    }
    for (idx, p) in ports.iter().enumerate() {
        if !is_valid_identifier(&p.mapping_key) {
            return Err(ValidationError::InvalidField {
                field: "ports.mapping_key",
                issue: "must match ^[a-z][a-z0-9-]{0,31}$",
            });
        }
        if p.public_port == 0 {
            return Err(ValidationError::InvalidField {
                field: "ports.public_port",
                issue: "must be 1..=65535",
            });
        }
        if let Some(earlier) = ports.get(..idx) {
            if earlier.iter().any(|prev| prev.mapping_key == p.mapping_key) {
                return Err(ValidationError::InvalidField {
                    field: "ports.mapping_key",
                    issue: "mapping_key must be unique",
                });
            }
        }
    }
    Ok(())
}

/// Request body for `POST /v1/tunnels/{id}/invitations` (`contracts/create-invitation-request.schema.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateInvitationRequest {
    /// Lifetime in seconds (`60..=604800`, `contracts.md` §15).
    pub expires_in_seconds: u32,
    /// Maximum claims (`1..=50`, `contracts.md` §15).
    pub max_claims: u8,
}

impl CreateInvitationRequest {
    /// Validate `contracts.md` §15 bounds.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !(60..=604_800).contains(&self.expires_in_seconds) {
            return Err(ValidationError::InvalidField {
                field: "expires_in_seconds",
                issue: "must be between 60 and 604800 seconds",
            });
        }
        if !(1..=50).contains(&self.max_claims) {
            return Err(ValidationError::InvalidField {
                field: "max_claims",
                issue: "must be between 1 and 50",
            });
        }
        Ok(())
    }
}

/// Response body for `POST /v1/tunnels/{id}/invitations` (`contracts/create-invitation-response.schema.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateInvitationResponse {
    /// Invitation UUID.
    pub id: String,
    /// Owning tunnel UUID.
    pub tunnel_id: String,
    /// One-time invitation secret (`^[A-Za-z0-9_-]{22,64}$`).
    pub secret: String,
    /// Maximum claims allowed (`1..=50`).
    pub max_claims: u8,
    /// Claims used so far (`0..=max_claims`).
    pub claim_count: u8,
    /// Creation timestamp (RFC 3339 UTC).
    pub created_at: String,
    /// Expiry timestamp (RFC 3339 UTC).
    pub expires_at: String,
}

impl CreateInvitationResponse {
    /// Validate [`CreateInvitationResponse`] against `contracts.md` §§5.11, 15.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !is_valid_uuid(&self.id) || !is_valid_uuid(&self.tunnel_id) {
            return Err(ValidationError::InvalidField {
                field: "id/tunnel_id",
                issue: "must be valid UUIDs",
            });
        }
        if !is_valid_invitation_secret(&self.secret) {
            return Err(ValidationError::InvalidField {
                field: "secret",
                issue: "must match ^[A-Za-z0-9_-]{22,64}$",
            });
        }
        if !(1..=50).contains(&self.max_claims) || self.claim_count > self.max_claims {
            return Err(ValidationError::InvalidField {
                field: "max_claims/claim_count",
                issue: "max_claims must be 1..=50 and claim_count <= max_claims",
            });
        }
        if !is_valid_rfc3339_utc(&self.created_at) || !is_valid_rfc3339_utc(&self.expires_at) {
            return Err(ValidationError::InvalidField {
                field: "created_at/expires_at",
                issue: "must be valid RFC 3339 UTC timestamps",
            });
        }
        Ok(())
    }
}

/// Request body for `POST /v1/invitations/claim` (`contracts/claim-invitation-request.schema.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimInvitationRequest {
    /// Invitation secret (`^[A-Za-z0-9_-]{22,64}$`).
    pub secret: String,
}

impl ClaimInvitationRequest {
    /// Validate [`ClaimInvitationRequest`] against `contracts.md` §§6, 15.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !is_valid_invitation_secret(&self.secret) {
            return Err(ValidationError::InvalidField {
                field: "secret",
                issue: "must match ^[A-Za-z0-9_-]{22,64}$",
            });
        }
        Ok(())
    }
}

/// Public relay endpoint and ports returned to a guest (`contracts.md` §6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimRelayDto {
    /// Relay region label.
    pub region: String,
    /// Relay public hostname.
    pub host: String,
    /// Public ports the guest's stock game client should connect to.
    pub ports: Vec<AllocatedPortDto>,
}

/// Response body for `POST /v1/invitations/claim` (`contracts/claim-invitation-response.schema.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimInvitationResponse {
    /// Tunnel UUID.
    pub tunnel_id: String,
    /// Game profile identifier.
    pub game_profile_id: String,
    /// Public relay host and ports.
    pub relay: ClaimRelayDto,
    /// Timestamp until which the guest's observed source IP is admitted (RFC 3339 UTC).
    pub admitted_until: String,
}

impl ClaimInvitationResponse {
    /// Validate [`ClaimInvitationResponse`] against `contracts.md` §6.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !is_valid_uuid(&self.tunnel_id) {
            return Err(ValidationError::InvalidField {
                field: "tunnel_id",
                issue: "must be a valid UUID",
            });
        }
        if !is_valid_identifier(&self.game_profile_id) {
            return Err(ValidationError::InvalidField {
                field: "game_profile_id",
                issue: "must match ^[a-z][a-z0-9-]{0,31}$",
            });
        }
        if self.relay.region.trim().is_empty() || self.relay.region.len() > 64 {
            return Err(ValidationError::InvalidField {
                field: "relay.region",
                issue: "must be 1-64 characters",
            });
        }
        if self.relay.host.trim().is_empty() || self.relay.host.len() > 253 {
            return Err(ValidationError::InvalidField {
                field: "relay.host",
                issue: "must be 1-253 characters",
            });
        }
        validate_allocated_ports(&self.relay.ports)?;
        if !is_valid_rfc3339_utc(&self.admitted_until) {
            return Err(ValidationError::InvalidField {
                field: "admitted_until",
                issue: "must be a valid RFC 3339 UTC timestamp",
            });
        }
        Ok(())
    }
}

/// Request body for `POST /v1/auth/exchange` (`contracts.md` §5.1, `api.md` #1, `T-040`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthExchangeRequest {
    /// Authorization code issued by identity provider (1–512 chars).
    pub code: String,
    /// RFC 7636 PKCE code verifier (43–128 unreserved characters).
    pub code_verifier: String,
    /// Registered redirect URI matching the initial authorization request.
    pub redirect_uri: String,
    /// Optional user-supplied device or client label (1–64 chars).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_label: Option<String>,
    /// Optional PKCE challenge method. If provided, must be `"S256"` (`ADR-018`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_challenge_method: Option<String>,
}

impl AuthExchangeRequest {
    /// Validate [`AuthExchangeRequest`] against `contracts.md` §5.1 and `ADR-018`.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.code.trim().is_empty() || self.code.len() > 512 {
            return Err(ValidationError::InvalidField {
                field: "code",
                issue: "must be 1-512 non-empty characters",
            });
        }
        if !is_valid_pkce_verifier(&self.code_verifier) {
            return Err(ValidationError::InvalidField {
                field: "code_verifier",
                issue: "must be 43-128 unreserved ASCII characters (RFC 7636)",
            });
        }
        if !is_valid_redirect_uri(&self.redirect_uri) {
            return Err(ValidationError::InvalidField {
                field: "redirect_uri",
                issue: "must be a valid URI (1-512 characters)",
            });
        }
        if let Some(label) = &self.device_label {
            if label.trim().is_empty() || label.len() > 64 {
                return Err(ValidationError::InvalidField {
                    field: "device_label",
                    issue: "must be 1-64 characters if provided",
                });
            }
        }
        if let Some(method) = &self.code_challenge_method {
            if method.eq_ignore_ascii_case("plain") {
                return Err(ValidationError::InvalidField {
                    field: "code_challenge_method",
                    issue: "plain PKCE method is prohibited; S256 is required (ADR-018)",
                });
            }
            if method != "S256" {
                return Err(ValidationError::InvalidField {
                    field: "code_challenge_method",
                    issue: "unsupported code_challenge_method (must be S256)",
                });
            }
        }
        Ok(())
    }
}

/// Request body for `POST /v1/auth/refresh` (`contracts.md` §5.2, `api.md` #2, `T-040`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthRefreshRequest {
    /// High-entropy refresh token issued by the server.
    pub refresh_token: String,
}

impl AuthRefreshRequest {
    /// Validate [`AuthRefreshRequest`] against `contracts.md` §5.2.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.refresh_token.trim().is_empty() || self.refresh_token.len() > 512 {
            return Err(ValidationError::InvalidField {
                field: "refresh_token",
                issue: "must be 1-512 non-empty characters",
            });
        }
        Ok(())
    }
}

/// Response body for `POST /v1/auth/exchange` and `POST /v1/auth/refresh` (`contracts.md` §§5.1, 5.2, `T-040`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthTokenResponse {
    /// Short-lived access token used for authenticated routes.
    pub access_token: String,
    /// Token type (always `"Bearer"`).
    pub token_type: String,
    /// Lifetime of access_token in seconds (e.g. `900`).
    pub expires_in: u64,
    /// High-entropy refresh token for session rotation.
    pub refresh_token: String,
}

impl AuthTokenResponse {
    /// Validate [`AuthTokenResponse`].
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.access_token.trim().is_empty() || self.access_token.len() > 4096 {
            return Err(ValidationError::InvalidField {
                field: "access_token",
                issue: "must be 1-4096 characters",
            });
        }
        if self.token_type != "Bearer" {
            return Err(ValidationError::InvalidField {
                field: "token_type",
                issue: "token_type must be Bearer",
            });
        }
        if self.expires_in == 0 {
            return Err(ValidationError::InvalidField {
                field: "expires_in",
                issue: "must be positive non-zero seconds",
            });
        }
        if self.refresh_token.trim().is_empty() || self.refresh_token.len() > 512 {
            return Err(ValidationError::InvalidField {
                field: "refresh_token",
                issue: "must be 1-512 non-empty characters",
            });
        }
        Ok(())
    }
}

/// Response body for `GET /v1/me` (`contracts.md` §5.4, `api.md` #4, `T-040`).
///
/// Contains NO sensitive personal data (`contracts.md` §5.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserProfileResponse {
    /// Canonical user identifier (UUID).
    pub id: String,
    /// Optional user display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Account status string (`"active"`, `"suspended"`).
    pub status: String,
}

impl UserProfileResponse {
    /// Validate [`UserProfileResponse`].
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !is_valid_uuid(&self.id) {
            return Err(ValidationError::InvalidField {
                field: "id",
                issue: "must be a valid UUID",
            });
        }
        if let Some(name) = &self.display_name {
            if name.len() > 64 {
                return Err(ValidationError::InvalidField {
                    field: "display_name",
                    issue: "must not exceed 64 characters",
                });
            }
        }
        if self.status != "active" && self.status != "suspended" {
            return Err(ValidationError::InvalidField {
                field: "status",
                issue: "must be active or suspended",
            });
        }
        Ok(())
    }
}
