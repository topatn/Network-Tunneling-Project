//! Message types, binary and length-prefixed framing, validation, and state machines shared
//! by the engine, the relay, and the control plane.
//!
//! Wire formats are defined in `contracts.md` §§3, 5, 6, 10–12, and 15, and mirrored by the
//! eleven JSON Schemas in `contracts/*.schema.json` and the conformance suite in
//! `contracts/examples.json` (T-031).
//!
//! ## Boundaries (`AGENTS.md` §3)
//!
//! * Never depends on the UI (`apps/desktop`) or any network/database runtime.
//! * Never accepts a host-local destination address or port (`INV-1`, `ADR-003`).
//! * Every parser and decoder bounds its input length before allocating and returns a typed
//!   error on malformed frames without panicking (`AGENTS.md` §4, `NET-007`, `TEST-001`).

pub mod control;
pub mod framing;
pub mod http;
pub mod state;
pub mod token;
pub mod validate;

use std::fmt;

pub use control::{
    decode_control_frame, encode_control_frame, ControlMessage, FlowSummaryMessage, GoodbyeMessage,
    HelloAckMapping, HelloAckMessage, HelloMessage, RejectCode, RejectMessage, SequenceMessage,
    SessionLimits, TokenRefreshAckMessage, TokenRefreshMessage, CONTROL_PREFIX_LEN,
    CURRENT_PROTOCOL_VERSION, MAX_CONTROL_FRAME_BYTES,
};
pub use framing::{
    encode_udp_datagram, FrameError, TcpOpenHeader, TcpOpenStatus, UdpDatagram,
    DEFAULT_MAX_DATAGRAM_BYTES, TCP_OPEN_FRAME_TYPE, TCP_OPEN_HEADER_LEN, UDP_HEADER_LEN,
};
pub use http::{
    AllocatedPortDto, AllocationDto, ApiErrorCode, AuthExchangeRequest, AuthRefreshRequest,
    AuthTokenResponse, ClaimInvitationRequest, ClaimInvitationResponse, ClaimRelayDto,
    CreateInvitationRequest, CreateInvitationResponse, CreateTunnelRequest, ErrorBody, ErrorDetail,
    ErrorResponse, IssueTunnelTokenResponse, MappingRequest, Protocol, RelayEndpointDto,
    StartTunnelResponse, Tunnel, TunnelMappingDto, TunnelState, UserProfileResponse,
    MAX_TUNNEL_MAPPINGS,
};
pub use state::{EngineSessionEvent, EngineSessionState, InvalidSessionTransition};
pub use token::{
    base64url_decode, base64url_encode, constant_time_eq_32, hmac_sha256, is_valid_kid,
    sha256_digest, sign_with_key, verify_allocation_token, AllocationTokenClaims,
    AllocationTokenHeader, KeyId, KeyLifecycleState, KeyRingError, ManagedSigningKey, RelayKeySet,
    RelayVerificationKey, SigningKeyMaterial, SigningKeyRing, TokenSignError,
    TokenVerificationError, DEFAULT_TOKEN_ISSUER, KEY_LEN_BYTES, MAX_KEYS_IN_RING,
    MAX_OVERLAP_WINDOW_SECS, MAX_TOKEN_LEN, TOKEN_ALG_HS256, TOKEN_TYP_ALLOC,
};
pub use validate::{
    compute_pkce_s256_challenge, is_local_target_field, is_valid_identifier,
    is_valid_invitation_secret, is_valid_pkce_verifier, is_valid_redirect_uri,
    is_valid_rfc3339_utc, is_valid_tunnel_name, is_valid_uuid, scan_json_for_local_target,
    ValidationError, FORBIDDEN_LOCAL_TARGET_STEMS, MAX_JSON_DEPTH,
};

/// Top-level error returned when validating a JSON payload against one of the `contracts/`
/// schemas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContractError {
    /// Unknown schema path.
    UnknownSchema(String),
    /// Deserialization failed (missing required field, wrong type, or unknown field).
    MalformedPayload(String),
    /// Domain, format, or `INV-1` rule failed.
    Validation(ValidationError),
}

impl fmt::Display for ContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownSchema(s) => write!(f, "unknown contract schema {s:?}"),
            Self::MalformedPayload(msg) => write!(f, "malformed payload: {msg}"),
            Self::Validation(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ContractError {}

/// Validate an untrusted JSON payload against one of the eleven `contracts/*.schema.json`
/// contracts.
///
/// Enforces:
/// 1. `INV-1` recursive local-target field rejection ([`scan_json_for_local_target`]),
/// 2. Strict `#[serde(deny_unknown_fields)]` structural and type deserialization, and
/// 3. Format (`uuid`, `date-time`) and cross-field rules from `contracts.md` §15.
pub fn validate_contract_payload(
    schema_path: &str,
    payload: &serde_json::Value,
) -> Result<(), ContractError> {
    scan_json_for_local_target(payload).map_err(ContractError::Validation)?;

    match schema_path {
        "contracts/error.schema.json" | "contracts/error-response.schema.json" => {
            let parsed: ErrorResponse = serde_json::from_value(payload.clone())
                .map_err(|e| ContractError::MalformedPayload(e.to_string()))?;
            parsed.validate().map_err(ContractError::Validation)
        }
        "contracts/create-tunnel-request.schema.json" => {
            let parsed: CreateTunnelRequest = serde_json::from_value(payload.clone())
                .map_err(|e| ContractError::MalformedPayload(e.to_string()))?;
            parsed.validate().map_err(ContractError::Validation)
        }
        "contracts/tunnel.schema.json" => {
            let parsed: Tunnel = serde_json::from_value(payload.clone())
                .map_err(|e| ContractError::MalformedPayload(e.to_string()))?;
            parsed.validate().map_err(ContractError::Validation)
        }
        "contracts/start-tunnel-response.schema.json" => {
            let parsed: StartTunnelResponse = serde_json::from_value(payload.clone())
                .map_err(|e| ContractError::MalformedPayload(e.to_string()))?;
            parsed.validate().map_err(ContractError::Validation)
        }
        "contracts/create-invitation-request.schema.json" => {
            let parsed: CreateInvitationRequest = serde_json::from_value(payload.clone())
                .map_err(|e| ContractError::MalformedPayload(e.to_string()))?;
            parsed.validate().map_err(ContractError::Validation)
        }
        "contracts/create-invitation-response.schema.json" => {
            let parsed: CreateInvitationResponse = serde_json::from_value(payload.clone())
                .map_err(|e| ContractError::MalformedPayload(e.to_string()))?;
            parsed.validate().map_err(ContractError::Validation)
        }
        "contracts/claim-invitation-request.schema.json" => {
            let parsed: ClaimInvitationRequest = serde_json::from_value(payload.clone())
                .map_err(|e| ContractError::MalformedPayload(e.to_string()))?;
            parsed.validate().map_err(ContractError::Validation)
        }
        "contracts/claim-invitation-response.schema.json" => {
            let parsed: ClaimInvitationResponse = serde_json::from_value(payload.clone())
                .map_err(|e| ContractError::MalformedPayload(e.to_string()))?;
            parsed.validate().map_err(ContractError::Validation)
        }
        "contracts/tunnel-hello.schema.json" => {
            let msg: ControlMessage = serde_json::from_value(payload.clone())
                .map_err(|e| ContractError::MalformedPayload(e.to_string()))?;
            if !matches!(msg, ControlMessage::Hello(_)) {
                return Err(ContractError::MalformedPayload(
                    "expected type == \"hello\"".to_owned(),
                ));
            }
            msg.validate().map_err(ContractError::Validation)
        }
        "contracts/tunnel-hello-ack.schema.json" => {
            let msg: ControlMessage = serde_json::from_value(payload.clone())
                .map_err(|e| ContractError::MalformedPayload(e.to_string()))?;
            if !matches!(msg, ControlMessage::HelloAck(_)) {
                return Err(ContractError::MalformedPayload(
                    "expected type == \"hello_ack\"".to_owned(),
                ));
            }
            msg.validate().map_err(ContractError::Validation)
        }
        other => Err(ContractError::UnknownSchema(other.to_owned())),
    }
}
