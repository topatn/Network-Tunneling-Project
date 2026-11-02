//! Binary data-path framing for TCP streams and UDP datagrams (`contracts.md` §11, `NET-007`).
//!
//! ## Wire formats (`contracts.md` §11)
//!
//! * **TCP stream open header (`relay -> engine`, 3 bytes):**
//!   * Byte 0: frame type `0x01` ([`TCP_OPEN_FRAME_TYPE`])
//!   * Bytes 1..3: `mapping_index` (`u16`, big-endian)
//! * **TCP stream open status (`engine -> relay`, 1 byte):**
//!   * `0x00` connected, `0x01` refused, `0x02` limit reached ([`TcpOpenStatus`])
//! * **UDP datagram (`either direction`, 6-byte header + payload):**
//!   * Bytes 0..2: `mapping_index` (`u16`, big-endian)
//!   * Bytes 2..6: `flow_id` (`u32`, big-endian)
//!   * Bytes 6..: raw UDP datagram payload

use std::fmt;

use crate::validate::ValidationError;

/// Frame type byte for opening a forwarded TCP stream (`contracts.md` §11).
pub const TCP_OPEN_FRAME_TYPE: u8 = 0x01;

/// Exact byte length of the TCP stream open header (`1 + 2 = 3`).
pub const TCP_OPEN_HEADER_LEN: usize = 3;

/// Exact byte length of the UDP datagram header (`2 + 4 = 6`).
pub const UDP_HEADER_LEN: usize = 6;

/// Default conservative QUIC maximum datagram frame size (1200 bytes) used when a transport
/// has not negotiated a larger limit.
pub const DEFAULT_MAX_DATAGRAM_BYTES: usize = 1200;

/// Errors returned when encoding or decoding control or data-path frames (`NET-007`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// Control frame had fewer than 4 bytes for its length prefix.
    TruncatedControlPrefix {
        /// Number of bytes available.
        actual_len: usize,
    },
    /// Control frame declared a zero length or a length above `max_len`.
    ControlFrameLengthOutOfBounds {
        /// Length declared in the 4-byte prefix.
        declared_len: usize,
        /// Maximum permitted payload length.
        max_len: usize,
    },
    /// Buffer did not contain all `declared_len` bytes of the control payload.
    TruncatedControlPayload {
        /// Declared payload length.
        declared_len: usize,
        /// Bytes actually present after the prefix.
        available_len: usize,
    },
    /// Control payload was not valid UTF-8 JSON matching [`crate::control::ControlMessage`].
    MalformedControlJson,
    /// Control payload decoded as JSON but failed a domain or `INV-1` rule.
    InvalidControlPayload(ValidationError),
    /// TCP open header was shorter than [`TCP_OPEN_HEADER_LEN`].
    TruncatedTcpOpenHeader {
        /// Actual byte count provided.
        actual_len: usize,
    },
    /// First byte of a TCP stream header was not [`TCP_OPEN_FRAME_TYPE`] (`0x01`).
    UnknownTcpFrameType(u8),
    /// Status byte returned on a TCP stream was not `0x00`, `0x01`, or `0x02`.
    UnknownTcpOpenStatus(u8),
    /// UDP datagram was shorter than [`UDP_HEADER_LEN`] (6 bytes).
    TruncatedUdpHeader {
        /// Actual byte count of the datagram.
        actual_len: usize,
    },
    /// `max_datagram_bytes` was configured smaller than [`UDP_HEADER_LEN`].
    InvalidDatagramLimit {
        /// Configured ceiling.
        max_datagram_bytes: usize,
    },
    /// UDP datagram exceeded `max_datagram_bytes` (`contracts.md` §11: dropped and counted in `datagrams_dropped`).
    OversizedDatagram {
        /// Total datagram size including the 6-byte header.
        datagram_len: usize,
        /// Maximum permitted datagram size.
        max_datagram_bytes: usize,
    },
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedControlPrefix { actual_len } => write!(
                f,
                "control frame prefix truncated: need {CONTROL_PREFIX_LEN} bytes, got {actual_len}",
                CONTROL_PREFIX_LEN = crate::control::CONTROL_PREFIX_LEN
            ),
            Self::ControlFrameLengthOutOfBounds {
                declared_len,
                max_len,
            } => write!(
                f,
                "control frame length {declared_len} is out of bounds (1..={max_len})"
            ),
            Self::TruncatedControlPayload {
                declared_len,
                available_len,
            } => write!(
                f,
                "control frame payload truncated: declared {declared_len} bytes, only {available_len} available"
            ),
            Self::MalformedControlJson => {
                f.write_str("control frame payload is not a valid control message")
            }
            Self::InvalidControlPayload(err) => write!(f, "control frame rejected: {err}"),
            Self::TruncatedTcpOpenHeader { actual_len } => write!(
                f,
                "TCP open header truncated: need {TCP_OPEN_HEADER_LEN} bytes, got {actual_len}"
            ),
            Self::UnknownTcpFrameType(byte) => {
                write!(f, "unknown TCP frame type 0x{byte:02x} (expected 0x01)")
            }
            Self::UnknownTcpOpenStatus(byte) => {
                write!(f, "unknown TCP open status byte 0x{byte:02x}")
            }
            Self::TruncatedUdpHeader { actual_len } => write!(
                f,
                "UDP datagram header truncated: need at least {UDP_HEADER_LEN} bytes, got {actual_len}"
            ),
            Self::InvalidDatagramLimit { max_datagram_bytes } => write!(
                f,
                "max_datagram_bytes ({max_datagram_bytes}) must be at least {UDP_HEADER_LEN}"
            ),
            Self::OversizedDatagram {
                datagram_len,
                max_datagram_bytes,
            } => write!(
                f,
                "UDP datagram of {datagram_len} bytes exceeds max_datagram_bytes ({max_datagram_bytes})"
            ),
        }
    }
}

impl std::error::Error for FrameError {}

/// 3-byte header sent by the relay when opening a forwarded TCP stream (`contracts.md` §11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TcpOpenHeader {
    /// Mapping index from `hello_ack.mappings`.
    pub mapping_index: u16,
}

impl TcpOpenHeader {
    /// Encode the 3-byte TCP stream open header.
    pub fn encode(self) -> [u8; TCP_OPEN_HEADER_LEN] {
        let [hi, lo] = self.mapping_index.to_be_bytes();
        [TCP_OPEN_FRAME_TYPE, hi, lo]
    }

    /// Decode a 3-byte TCP stream open header from the front of `bytes`.
    pub fn decode(bytes: &[u8]) -> Result<Self, FrameError> {
        let Some(&frame_type) = bytes.first() else {
            return Err(FrameError::TruncatedTcpOpenHeader {
                actual_len: bytes.len(),
            });
        };
        let Some(&hi) = bytes.get(1) else {
            return Err(FrameError::TruncatedTcpOpenHeader {
                actual_len: bytes.len(),
            });
        };
        let Some(&lo) = bytes.get(2) else {
            return Err(FrameError::TruncatedTcpOpenHeader {
                actual_len: bytes.len(),
            });
        };
        if frame_type != TCP_OPEN_FRAME_TYPE {
            return Err(FrameError::UnknownTcpFrameType(frame_type));
        }
        Ok(Self {
            mapping_index: u16::from_be_bytes([hi, lo]),
        })
    }
}

/// 1-byte status reply sent by the engine after attempting to dial the local TCP target (`contracts.md` §11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpOpenStatus {
    /// `0x00`: local TCP target connected; bidirectional byte forwarding begins.
    Connected,
    /// `0x01`: refused (unknown or disabled mapping, local target policy denial, or dial failure).
    Refused,
    /// `0x02`: per-session TCP flow limit reached.
    LimitReached,
}

impl TcpOpenStatus {
    /// Convert status to its wire byte (`0x00`, `0x01`, or `0x02`).
    pub fn to_byte(self) -> u8 {
        match self {
            Self::Connected => 0x00,
            Self::Refused => 0x01,
            Self::LimitReached => 0x02,
        }
    }

    /// Parse status from its wire byte.
    pub fn from_byte(byte: u8) -> Result<Self, FrameError> {
        match byte {
            0x00 => Ok(Self::Connected),
            0x01 => Ok(Self::Refused),
            0x02 => Ok(Self::LimitReached),
            other => Err(FrameError::UnknownTcpOpenStatus(other)),
        }
    }
}

/// Borrowed view of a framed UDP datagram (`contracts.md` §11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UdpDatagram<'a> {
    /// Mapping index from `hello_ack.mappings` (big-endian `u16`).
    pub mapping_index: u16,
    /// Relay-assigned flow identifier per `(mapping, guest source address and port)` (big-endian `u32`).
    pub flow_id: u32,
    /// Raw UDP payload bytes following the 6-byte header.
    pub payload: &'a [u8],
}

impl<'a> UdpDatagram<'a> {
    /// Decode a QUIC DATAGRAM frame against `max_datagram_bytes` (`contracts.md` §11).
    ///
    /// Returns [`FrameError::TruncatedUdpHeader`] if `frame.len() < 6`, and
    /// [`FrameError::OversizedDatagram`] if `frame.len() > max_datagram_bytes`.
    pub fn decode(frame: &'a [u8], max_datagram_bytes: usize) -> Result<Self, FrameError> {
        if max_datagram_bytes < UDP_HEADER_LEN {
            return Err(FrameError::InvalidDatagramLimit { max_datagram_bytes });
        }
        if frame.len() > max_datagram_bytes {
            return Err(FrameError::OversizedDatagram {
                datagram_len: frame.len(),
                max_datagram_bytes,
            });
        }
        let Some(&m0) = frame.first() else {
            return Err(FrameError::TruncatedUdpHeader {
                actual_len: frame.len(),
            });
        };
        let Some(&m1) = frame.get(1) else {
            return Err(FrameError::TruncatedUdpHeader {
                actual_len: frame.len(),
            });
        };
        let Some(&f0) = frame.get(2) else {
            return Err(FrameError::TruncatedUdpHeader {
                actual_len: frame.len(),
            });
        };
        let Some(&f1) = frame.get(3) else {
            return Err(FrameError::TruncatedUdpHeader {
                actual_len: frame.len(),
            });
        };
        let Some(&f2) = frame.get(4) else {
            return Err(FrameError::TruncatedUdpHeader {
                actual_len: frame.len(),
            });
        };
        let Some(&f3) = frame.get(5) else {
            return Err(FrameError::TruncatedUdpHeader {
                actual_len: frame.len(),
            });
        };
        let payload = frame.get(UDP_HEADER_LEN..).unwrap_or(&[]);
        Ok(Self {
            mapping_index: u16::from_be_bytes([m0, m1]),
            flow_id: u32::from_be_bytes([f0, f1, f2, f3]),
            payload,
        })
    }
}

/// Encode a UDP datagram with its 6-byte header (`mapping_index` + `flow_id` + `payload`),
/// enforcing `max_datagram_bytes` before allocating (`contracts.md` §11).
pub fn encode_udp_datagram(
    mapping_index: u16,
    flow_id: u32,
    payload: &[u8],
    max_datagram_bytes: usize,
) -> Result<Vec<u8>, FrameError> {
    if max_datagram_bytes < UDP_HEADER_LEN {
        return Err(FrameError::InvalidDatagramLimit { max_datagram_bytes });
    }
    let total_len = UDP_HEADER_LEN.saturating_add(payload.len());
    if total_len > max_datagram_bytes {
        return Err(FrameError::OversizedDatagram {
            datagram_len: total_len,
            max_datagram_bytes,
        });
    }
    let mut out = Vec::with_capacity(total_len);
    out.extend_from_slice(&mapping_index.to_be_bytes());
    out.extend_from_slice(&flow_id.to_be_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}
