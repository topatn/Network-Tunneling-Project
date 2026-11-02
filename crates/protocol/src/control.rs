//! Tunnel control-stream messages and bounded length-prefixed JSON framing (`contracts.md` §10).
//!
//! ## Bounded framing (`AGENTS.md` §4, `NET-007`)
//!
//! The control stream carries length-prefixed UTF-8 JSON messages over the first bidirectional
//! QUIC stream (`contracts.md` §10). An untrusted peer could send a `u32::MAX` length prefix to
//! trigger a 4 GiB allocation. [`decode_control_frame`] checks the 4-byte big-endian length
//! against `1..=MAX_CONTROL_FRAME_BYTES` **before** touching or allocating for the payload.

use serde::{Deserialize, Serialize};

use crate::framing::FrameError;
use crate::http::{Protocol, MAX_TUNNEL_MAPPINGS};
use crate::validate::{
    is_valid_identifier, is_valid_rfc3339_utc, is_valid_uuid, scan_json_for_local_target,
    ValidationError,
};

/// Maximum control-stream JSON payload size in bytes (64 KiB).
pub const MAX_CONTROL_FRAME_BYTES: usize = 64 * 1024;

/// Length of the big-endian `u32` prefix on a control-stream frame.
pub const CONTROL_PREFIX_LEN: usize = 4;

/// Current tunnel wire protocol version (`contracts.md` §10).
pub const CURRENT_PROTOCOL_VERSION: u16 = 1;

/// `hello` message (`engine -> relay`, `contracts/tunnel-hello.schema.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelloMessage {
    /// Wire protocol version (`>= 1`).
    pub protocol_version: u16,
    /// Signed opaque allocation token.
    pub token: String,
    /// Engine client version string.
    pub client_version: String,
}

impl HelloMessage {
    /// Validate [`HelloMessage`] fields.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.protocol_version == 0 {
            return Err(ValidationError::InvalidField {
                field: "protocol_version",
                issue: "must be >= 1",
            });
        }
        if self.token.trim().is_empty() || self.token.len() > 4096 {
            return Err(ValidationError::InvalidField {
                field: "token",
                issue: "must be 1-4096 characters",
            });
        }
        if self.client_version.trim().is_empty() || self.client_version.len() > 64 {
            return Err(ValidationError::InvalidField {
                field: "client_version",
                issue: "must be 1-64 characters",
            });
        }
        Ok(())
    }
}

/// Per-session flow limits inside [`HelloAckMessage`] (`contracts.md` §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionLimits {
    /// Maximum concurrent TCP streams (`1..=4096`).
    pub max_tcp_flows: u16,
    /// Maximum concurrent UDP flow entries (`1..=4096`).
    pub max_udp_flows: u16,
}

/// One mapping entry in [`HelloAckMessage`] defining its data-path `index` (`contracts.md` §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelloAckMapping {
    /// Data-path mapping index used in TCP open headers and UDP datagram headers (`contracts.md` §11).
    pub index: u16,
    /// Profile mapping key.
    pub mapping_key: String,
    /// Transport protocol.
    pub protocol: Protocol,
    /// Relay public port (`1..=65535`).
    pub public_port: u16,
}

/// `hello_ack` message (`relay -> engine`, `contracts/tunnel-hello-ack.schema.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelloAckMessage {
    /// Session UUID.
    pub session_id: String,
    /// Heartbeat interval in milliseconds (`100..=300_000`).
    pub heartbeat_interval_ms: u32,
    /// Idle timeout in milliseconds (`1_000..=600_000`).
    pub idle_timeout_ms: u32,
    /// Per-session flow ceilings.
    pub limits: SessionLimits,
    /// Active mappings with their wire indexes.
    pub mappings: Vec<HelloAckMapping>,
}

impl HelloAckMessage {
    /// Validate [`HelloAckMessage`] against `contracts.md` §10.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !is_valid_uuid(&self.session_id) {
            return Err(ValidationError::InvalidField {
                field: "session_id",
                issue: "must be a valid UUID",
            });
        }
        if !(100..=300_000).contains(&self.heartbeat_interval_ms) {
            return Err(ValidationError::InvalidField {
                field: "heartbeat_interval_ms",
                issue: "must be between 100 and 300000 ms",
            });
        }
        if !(1_000..=600_000).contains(&self.idle_timeout_ms)
            || self.idle_timeout_ms <= self.heartbeat_interval_ms
        {
            return Err(ValidationError::InvalidField {
                field: "idle_timeout_ms",
                issue: "must be 1000..=600000 ms and strictly greater than heartbeat_interval_ms",
            });
        }
        if !(1..=4096).contains(&self.limits.max_tcp_flows)
            || !(1..=4096).contains(&self.limits.max_udp_flows)
        {
            return Err(ValidationError::InvalidField {
                field: "limits",
                issue: "max_tcp_flows and max_udp_flows must be 1..=4096",
            });
        }
        if !(1..=MAX_TUNNEL_MAPPINGS).contains(&self.mappings.len()) {
            return Err(ValidationError::InvalidField {
                field: "mappings",
                issue: "must contain 1 to 8 mappings",
            });
        }
        for (pos, m) in self.mappings.iter().enumerate() {
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
            if let Some(earlier) = self.mappings.get(..pos) {
                if earlier
                    .iter()
                    .any(|prev| prev.index == m.index || prev.mapping_key == m.mapping_key)
                {
                    return Err(ValidationError::InvalidField {
                        field: "mappings",
                        issue: "mapping index and mapping_key must be unique within hello_ack",
                    });
                }
            }
        }
        Ok(())
    }
}

/// Rejection codes sent by the relay in a `reject` control message (`contracts.md` §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectCode {
    /// Signature, audience, or claims invalid (terminal: transitions engine to `Failed`).
    TokenInvalid,
    /// Allocation token has expired (retryable after refreshing via `POST /v1/tunnels/{id}/token`).
    TokenExpired,
    /// Relay does not yet have or no longer has the reservation (retryable).
    AllocationNotFound,
    /// `protocol_version` is not supported by the relay (terminal: transitions engine to `Failed`).
    VersionUnsupported,
    /// Relay capacity reached (retryable).
    Capacity,
    /// Relay is draining (retryable).
    RelayDraining,
}

impl RejectCode {
    /// Whether the engine session state machine (`contracts.md` §12) transitions to
    /// `Reconnecting` (`true`) or terminal `Failed` (`false`) on this rejection code.
    pub fn is_retryable(self) -> bool {
        !matches!(self, Self::TokenInvalid | Self::VersionUnsupported)
    }
}

/// `reject` message (`relay -> engine`, `contracts.md` §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RejectMessage {
    /// Why the session was refused.
    pub code: RejectCode,
    /// Optional backoff hint in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

/// `ping` or `pong` message (`either direction`, `contracts.md` §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SequenceMessage {
    /// Monotonic sequence number for RTT sampling and liveness.
    pub seq: u64,
}

/// `token_refresh` message (`engine -> relay`, `contracts.md` §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenRefreshMessage {
    /// Fresh signed allocation token.
    pub token: String,
}

/// `token_refresh_ack` message (`relay -> engine`, `contracts.md` §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenRefreshAckMessage {
    /// Updated session expiry timestamp (RFC 3339 UTC).
    pub expires_at: String,
}

/// `flow_summary` message (`relay -> engine`, `contracts.md` §10).
///
/// Drives the "players seen" indicator (`architecture.md` §12: traffic observed, not confirmed
/// in-game presence).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowSummaryMessage {
    /// Active TCP streams.
    pub tcp_flows: u32,
    /// Active UDP flows.
    pub udp_flows: u32,
    /// Distinct guest source addresses observed.
    pub distinct_sources: u32,
}

/// `goodbye` message (`either direction`, `contracts.md` §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoodbyeMessage {
    /// Short human-readable or machine-readable close reason.
    pub reason: String,
}

/// Tagged union of all tunnel control-stream messages (`contracts.md` §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    /// Initial handshake from engine to relay.
    Hello(HelloMessage),
    /// Handshake acknowledgment from relay to engine.
    HelloAck(HelloAckMessage),
    /// Handshake rejection from relay to engine.
    Reject(RejectMessage),
    /// Liveness probe.
    Ping(SequenceMessage),
    /// Liveness reply.
    Pong(SequenceMessage),
    /// In-session allocation token refresh from engine to relay.
    TokenRefresh(TokenRefreshMessage),
    /// Token refresh acknowledgment from relay to engine.
    TokenRefreshAck(TokenRefreshAckMessage),
    /// Observed guest traffic summary from relay to engine.
    FlowSummary(FlowSummaryMessage),
    /// Graceful session close.
    Goodbye(GoodbyeMessage),
}

impl ControlMessage {
    /// Validate domain constraints on any [`ControlMessage`] variant.
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Hello(m) => m.validate(),
            Self::HelloAck(m) => m.validate(),
            Self::Reject(m) => {
                if let Some(ms) = m.retry_after_ms {
                    if ms > 3_600_000 {
                        return Err(ValidationError::InvalidField {
                            field: "retry_after_ms",
                            issue: "must be <= 3600000 ms",
                        });
                    }
                }
                Ok(())
            }
            Self::Ping(_) | Self::Pong(_) | Self::FlowSummary(_) => Ok(()),
            Self::TokenRefresh(m) => {
                if m.token.trim().is_empty() || m.token.len() > 4096 {
                    return Err(ValidationError::InvalidField {
                        field: "token",
                        issue: "must be 1-4096 characters",
                    });
                }
                Ok(())
            }
            Self::TokenRefreshAck(m) => {
                if !is_valid_rfc3339_utc(&m.expires_at) {
                    return Err(ValidationError::InvalidField {
                        field: "expires_at",
                        issue: "must be a valid RFC 3339 UTC timestamp",
                    });
                }
                Ok(())
            }
            Self::Goodbye(m) => {
                if m.reason.trim().is_empty()
                    || m.reason.len() > 128
                    || m.reason.chars().any(char::is_control)
                {
                    return Err(ValidationError::InvalidField {
                        field: "reason",
                        issue: "must be 1-128 characters with no control characters",
                    });
                }
                Ok(())
            }
        }
    }
}

/// Encode a validated [`ControlMessage`] into a length-prefixed frame (`4-byte BE u32 + JSON`).
pub fn encode_control_frame(msg: &ControlMessage) -> Result<Vec<u8>, FrameError> {
    msg.validate().map_err(FrameError::InvalidControlPayload)?;
    let json_bytes = serde_json::to_vec(msg).map_err(|_| FrameError::MalformedControlJson)?;
    if json_bytes.is_empty() || json_bytes.len() > MAX_CONTROL_FRAME_BYTES {
        return Err(FrameError::ControlFrameLengthOutOfBounds {
            declared_len: json_bytes.len(),
            max_len: MAX_CONTROL_FRAME_BYTES,
        });
    }
    let len_u32 =
        u32::try_from(json_bytes.len()).map_err(|_| FrameError::ControlFrameLengthOutOfBounds {
            declared_len: json_bytes.len(),
            max_len: MAX_CONTROL_FRAME_BYTES,
        })?;
    let mut out = Vec::with_capacity(CONTROL_PREFIX_LEN + json_bytes.len());
    out.extend_from_slice(&len_u32.to_be_bytes());
    out.extend_from_slice(&json_bytes);
    Ok(out)
}

/// Decode one length-prefixed [`ControlMessage`] from the front of `buf`, returning the
/// validated message and the total number of bytes consumed (`4 + payload_len`).
///
/// Rejects zero-length frames, frames declaring more than [`MAX_CONTROL_FRAME_BYTES`], truncated
/// frames, `INV-1` local-target fields, unknown fields, and invalid message payloads without
/// panicking (`NET-007`).
pub fn decode_control_frame(buf: &[u8]) -> Result<(ControlMessage, usize), FrameError> {
    let Some(prefix) = buf.get(..CONTROL_PREFIX_LEN) else {
        return Err(FrameError::TruncatedControlPrefix {
            actual_len: buf.len(),
        });
    };
    let Some(&b0) = prefix.first() else {
        return Err(FrameError::TruncatedControlPrefix {
            actual_len: buf.len(),
        });
    };
    let b1 = prefix.get(1).copied().unwrap_or(0);
    let b2 = prefix.get(2).copied().unwrap_or(0);
    let b3 = prefix.get(3).copied().unwrap_or(0);
    let declared_len = u32::from_be_bytes([b0, b1, b2, b3]) as usize;

    if declared_len == 0 || declared_len > MAX_CONTROL_FRAME_BYTES {
        return Err(FrameError::ControlFrameLengthOutOfBounds {
            declared_len,
            max_len: MAX_CONTROL_FRAME_BYTES,
        });
    }

    let total_len = CONTROL_PREFIX_LEN.saturating_add(declared_len);
    let Some(payload) = buf.get(CONTROL_PREFIX_LEN..total_len) else {
        return Err(FrameError::TruncatedControlPayload {
            declared_len,
            available_len: buf.len().saturating_sub(CONTROL_PREFIX_LEN),
        });
    };

    let raw_value: serde_json::Value =
        serde_json::from_slice(payload).map_err(|_| FrameError::MalformedControlJson)?;
    scan_json_for_local_target(&raw_value).map_err(FrameError::InvalidControlPayload)?;

    let msg: ControlMessage =
        serde_json::from_value(raw_value).map_err(|_| FrameError::MalformedControlJson)?;
    msg.validate().map_err(FrameError::InvalidControlPayload)?;
    Ok((msg, total_len))
}
