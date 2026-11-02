//! Local server reachability probes (`T-072`, `FR-011`, `contracts.md` §13).
//!
//! ## Probe semantics (`FR-011`, `research.md` §5 RQ-13.2c)
//!
//! * **TCP mappings (`ProbeLevel::L0`):** Connect-only TCP handshake (`TcpStream::connect_timeout`).
//!   Per `FR-011` and `research.md` §5 RQ-13.2c, Minecraft Java and Terraria servers send nothing on
//!   initial connect until the client speaks first, so TCP reachability must be determined from the
//!   TCP handshake and must never wait for server-initiated bytes.
//! * **UDP mappings (`ProbeLevel::L1`):** `UdpSocket::connect` only sets a kernel default peer and
//!   sends no packets, so a connect-only check would falsely report every UDP port as reachable.
//!   Therefore:
//!   1. If a profile-defined UDP query exists for `(profile_id, mapping_key)` (RakNet Unconnected
//!      Ping `0x01 -> 0x1c` for `minecraft-bedrock`, Source `A2S_INFO` for `l4d2`/`l4d1`, or UT3
//!      Query handshake `\xfe\xfd\x09` for `minecraft-java` `query`), the engine sends the query
//!      datagram and waits up to `timeout` for a valid response signature.
//!   2. If a valid response arrives, the probe reports [`ProbeStatus::Reachable`].
//!   3. If no response arrives (timeout or ICMP port unreachable) or the response is malformed, the
//!      probe reports [`ProbeStatus::Unreachable`].
//!   4. If no UDP probe is defined for `(profile_id, mapping_key)` (e.g., `terraria` `game-udp`,
//!      `l4d2` `client`, or an arbitrary third-party UDP profile), the probe reports
//!      [`ProbeStatus::Unknown`].
//!   5. **Invariant (`FR-011`):** A UDP probe **never** reports [`ProbeStatus::Reachable`] without
//!      receiving a verified response packet from the local server.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, UdpSocket};
use std::time::Duration;

use gtt_protocol::Protocol;
use serde::Deserialize;

use crate::target_policy::{ValidatedLocalTargetTable, ValidatedMappingTarget};

/// Default timeout for a local reachability probe (`500 ms`, `AGENTS.md` §4).
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// RakNet offline message magic (`16 bytes`) used by Minecraft Bedrock Unconnected Ping/Pong.
pub const RAKNET_OFFLINE_MAGIC: [u8; 16] = [
    0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78,
];

/// Source Engine `A2S_INFO` request payload (`\xff\xff\xff\xffTSource Engine Query\x00`).
pub const SOURCE_A2S_INFO_REQUEST: &[u8] = b"\xff\xff\xff\xffTSource Engine Query\x00";

///Minecraft Java UT3 Query handshake request (`\xfe\xfd\x09` + 4-byte session ID).
pub const MC_JAVA_QUERY_HANDSHAKE_REQUEST: [u8; 7] = [0xfe, 0xfd, 0x09, 0x00, 0x00, 0x00, 0x01];

/// Compatibility / probe depth level (`contracts.md` §13, `research.md` ADR-011).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeLevel {
    /// Transport-level reachability (TCP 3-way handshake).
    L0,
    /// Application-protocol status query/response (UDP query probe).
    L1,
}

impl ProbeLevel {
    /// Wire label for UI events (`"L0"` or `"L1"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::L0 => "L0",
            Self::L1 => "L1",
        }
    }
}

/// Reachability outcome for a single mapping (`FR-011`, `T-072`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeStatus {
    /// Local server accepted a TCP connection or answered a profile-defined UDP probe.
    Reachable,
    /// Local server refused the connection, timed out, or returned an invalid UDP response.
    Unreachable,
    /// Mapping is UDP and no profile-defined UDP query exists for it (`FR-011`).
    Unknown,
}

impl ProbeStatus {
    /// Wire string representation (`"reachable"`, `"unreachable"`, `"unknown"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reachable => "reachable",
            Self::Unreachable => "unreachable",
            Self::Unknown => "unknown",
        }
    }
}

/// Profile-defined UDP query protocol (`T-072`, `FR-011`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UdpProbeKind {
    /// Minecraft Bedrock RakNet Unconnected Ping (`0x01`) -> Unconnected Pong (`0x1c`).
    RakNetUnconnectedPing,
    /// Source Engine (`l4d2` / `l4d1`) `A2S_INFO` query -> `0x49` (`'I'`) or `0x41` (`'A'`) challenge.
    SourceA2sInfo,
    /// Minecraft Java UT3 Query (`\xfe\xfd\x09`) -> `\x09` handshake response.
    MinecraftJavaQueryHandshake,
}

impl UdpProbeKind {
    /// Look up the profile-defined UDP probe for `(profile_id, mapping_key)`, if one exists.
    pub fn for_profile_mapping(profile_id: &str, mapping_key: &str) -> Option<Self> {
        match (profile_id, mapping_key) {
            ("minecraft-bedrock", "game" | "game-ipv6") => Some(Self::RakNetUnconnectedPing),
            ("l4d2" | "l4d1", "game" | "sourcetv") => Some(Self::SourceA2sInfo),
            ("minecraft-java", "query") => Some(Self::MinecraftJavaQueryHandshake),
            _ => None,
        }
    }

    /// Build the wire request datagram for this UDP probe.
    pub fn build_request_packet(self) -> Vec<u8> {
        match self {
            Self::RakNetUnconnectedPing => {
                // 0x01 (1B) + client timestamp (8B) + RAKNET_OFFLINE_MAGIC (16B) + client GUID (8B) = 33B
                let mut pkt = Vec::with_capacity(33);
                pkt.push(0x01);
                pkt.extend_from_slice(&1_000_u64.to_be_bytes());
                pkt.extend_from_slice(&RAKNET_OFFLINE_MAGIC);
                pkt.extend_from_slice(&0x0102_0304_0506_0708_u64.to_be_bytes());
                pkt
            }
            Self::SourceA2sInfo => SOURCE_A2S_INFO_REQUEST.to_vec(),
            Self::MinecraftJavaQueryHandshake => MC_JAVA_QUERY_HANDSHAKE_REQUEST.to_vec(),
        }
    }

    /// Validate whether `response` is a genuine reply for this UDP probe (`FR-011`).
    pub fn validate_response(self, response: &[u8]) -> bool {
        match self {
            Self::RakNetUnconnectedPing => {
                // Unconnected Pong (0x1c): 1B id + 8B time + 8B server GUID + 16B magic = at least 33 bytes
                let Some(&first) = response.first() else {
                    return false;
                };
                if first != 0x1c || response.len() < 33 {
                    return false;
                }
                response
                    .get(17..33)
                    .is_some_and(|magic| magic == RAKNET_OFFLINE_MAGIC)
            }
            Self::SourceA2sInfo => {
                // 4-byte 0xFF_FF_FF_FF header + 'I' (0x49 info) or 'A' (0x41 challenge)
                response.len() >= 5
                    && response.get(0..4) == Some(&[0xff, 0xff, 0xff, 0xff])
                    && matches!(response.get(4), Some(0x49 | 0x41))
            }
            Self::MinecraftJavaQueryHandshake => {
                // 0x09 type + 4-byte session ID + at least 1 byte of challenge string
                response.len() >= 6 && response.first() == Some(&0x09)
            }
        }
    }
}

/// Structured result of probing a single local mapping (`contracts.md` §13 `local_probe_result`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalProbeResult {
    /// Profile mapping key (`"game"`, `"query"`, `"rcon"`, etc.).
    pub mapping_key: String,
    /// Probe level (`L0` for TCP connect, `L1` for UDP query).
    pub level: ProbeLevel,
    /// Reachability status (`Reachable`, `Unreachable`, or `Unknown`).
    pub status: ProbeStatus,
    /// Convenience boolean matching `contracts.md` §13 (`true` iff `status == ProbeStatus::Reachable`).
    pub ok: bool,
    /// Human-readable diagnostic detail.
    pub detail: String,
}

/// Execute a connect-only TCP reachability probe against `target` (`FR-011`, `T-072`).
pub fn probe_tcp_connect(
    mapping_key: impl Into<String>,
    target: SocketAddr,
    timeout: Duration,
) -> LocalProbeResult {
    let key = mapping_key.into();
    let bounded_timeout = clamp_probe_timeout(timeout);
    match TcpStream::connect_timeout(&target, bounded_timeout) {
        Ok(stream) => {
            let _ = stream.shutdown(std::net::Shutdown::Both);
            LocalProbeResult {
                mapping_key: key,
                level: ProbeLevel::L0,
                status: ProbeStatus::Reachable,
                ok: true,
                detail: format!("TCP connect succeeded to {target}"),
            }
        }
        Err(err) => LocalProbeResult {
            mapping_key: key,
            level: ProbeLevel::L0,
            status: ProbeStatus::Unreachable,
            ok: false,
            detail: format!("TCP connect failed to {target}: {err}"),
        },
    }
}

/// Execute a UDP reachability probe for `(profile_id, mapping_key)` against `target` (`FR-011`,
/// `T-072`).
///
/// Never reports [`ProbeStatus::Reachable`] unless a profile-defined UDP probe exists AND the
/// server returns a valid response within `timeout`.
pub fn probe_udp_mapping(
    profile_id: &str,
    mapping_key: impl Into<String>,
    target: SocketAddr,
    timeout: Duration,
) -> LocalProbeResult {
    let key = mapping_key.into();
    let Some(probe_kind) = UdpProbeKind::for_profile_mapping(profile_id, &key) else {
        let detail =
            format!("no profile-defined UDP probe for profile '{profile_id}' mapping '{key}'");
        return LocalProbeResult {
            mapping_key: key,
            level: ProbeLevel::L1,
            status: ProbeStatus::Unknown,
            ok: false,
            detail,
        };
    };

    let bounded_timeout = clamp_probe_timeout(timeout);
    let bind_addr = match target {
        SocketAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        SocketAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 0),
    };

    let socket = match UdpSocket::bind(bind_addr) {
        Ok(s) => s,
        Err(err) => {
            return LocalProbeResult {
                mapping_key: key,
                level: ProbeLevel::L1,
                status: ProbeStatus::Unreachable,
                ok: false,
                detail: format!("failed to bind local UDP probe socket: {err}"),
            };
        }
    };

    let _ = socket.set_read_timeout(Some(bounded_timeout));
    let _ = socket.set_write_timeout(Some(bounded_timeout));

    let packet = probe_kind.build_request_packet();
    if let Err(err) = socket.send_to(&packet, target) {
        return LocalProbeResult {
            mapping_key: key,
            level: ProbeLevel::L1,
            status: ProbeStatus::Unreachable,
            ok: false,
            detail: format!("UDP probe send to {target} failed: {err}"),
        };
    }

    let mut recv_buf = [0_u8; 1500];
    match socket.recv_from(&mut recv_buf) {
        Ok((n, from_addr)) => {
            let slice = recv_buf.get(..n).unwrap_or(&[]);
            if from_addr.port() == target.port() && probe_kind.validate_response(slice) {
                LocalProbeResult {
                    mapping_key: key,
                    level: ProbeLevel::L1,
                    status: ProbeStatus::Reachable,
                    ok: true,
                    detail: format!("received valid {probe_kind:?} response from {target}"),
                }
            } else {
                LocalProbeResult {
                    mapping_key: key,
                    level: ProbeLevel::L1,
                    status: ProbeStatus::Unreachable,
                    ok: false,
                    detail: format!("received invalid UDP probe response from {from_addr}"),
                }
            }
        }
        Err(err) => LocalProbeResult {
            mapping_key: key,
            level: ProbeLevel::L1,
            status: ProbeStatus::Unreachable,
            ok: false,
            detail: format!("no UDP probe response from {target}: {err}"),
        },
    }
}

/// Probe a single [`ValidatedMappingTarget`] (`FR-011`, `T-072`).
pub fn probe_validated_target(
    profile_id: &str,
    target: &ValidatedMappingTarget,
    timeout: Duration,
) -> LocalProbeResult {
    match target.protocol() {
        Protocol::Tcp => probe_tcp_connect(target.mapping_key(), target.local_addr(), timeout),
        Protocol::Udp => probe_udp_mapping(
            profile_id,
            target.mapping_key(),
            target.local_addr(),
            timeout,
        ),
    }
}

/// Probe either a specific `mapping_key` or all enabled mappings in `table` (`contracts.md` §13
/// `probe_local`).
pub fn probe_local_targets(
    table: &ValidatedLocalTargetTable,
    mapping_key: Option<&str>,
    timeout: Duration,
) -> Vec<LocalProbeResult> {
    let profile_id = table.profile_id();
    match mapping_key {
        Some(key) => table
            .get_by_key(key)
            .map(|target| vec![probe_validated_target(profile_id, target, timeout)])
            .unwrap_or_default(),
        None => table
            .targets()
            .into_iter()
            .filter(|t| t.enabled())
            .map(|target| probe_validated_target(profile_id, target, timeout))
            .collect(),
    }
}

/// Maximum accepted byte length for a Minecraft Java Server List Ping status response (`AGENTS.md` §4).
pub const MAX_MC_SLP_STATUS_BYTES: usize = 32_768;

/// Maximum server hostname length in a Minecraft Java SLP handshake packet (`AGENTS.md` §4).
pub const MAX_MC_SLP_HOST_BYTES: usize = 255;

/// Parsed version block from a Minecraft: Java Edition Server List Ping JSON response (`T-090`,
/// `GAME-002`, `TEST-005`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MinecraftJavaStatusVersion {
    /// Human-readable server version string (e.g. `"26.3"` or `"1.21.4"`).
    pub name: String,
    /// Protocol version integer (e.g. `770`).
    pub protocol: u32,
}

/// Parsed players block from a Minecraft: Java Edition Server List Ping JSON response (`T-090`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MinecraftJavaStatusPlayers {
    /// Configured maximum player slots (`max-players`).
    pub max: u32,
    /// Currently connected player count.
    pub online: u32,
}

/// Parsed Minecraft: Java Edition Server List Ping JSON status payload (`T-090`, `GAME-002`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MinecraftJavaStatusResponse {
    /// Version metadata reported by the server.
    pub version: MinecraftJavaStatusVersion,
    /// Player count metadata reported by the server.
    pub players: MinecraftJavaStatusPlayers,
}

/// Complete L1 probe report for a Minecraft: Java Edition Server List Ping handshake + status +
/// ping/pong exchange (`T-090`, `GAME-002`, `TEST-005`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinecraftJavaSlpProbeReport {
    /// Structured `L1` probe result.
    pub probe: LocalProbeResult,
    /// Parsed status payload (`version` and `players`).
    pub status: MinecraftJavaStatusResponse,
    /// Whether the `0x01` Pong echoed the exact 8-byte ping payload.
    pub pong_matched: bool,
}

/// Encode a `u32` as a Minecraft protocol `VarInt` (1 to 5 bytes).
pub fn encode_mc_varint(mut value: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(5);
    for _ in 0..5 {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
            out.push(byte);
        } else {
            out.push(byte);
            break;
        }
    }
    out
}

/// Decode a Minecraft protocol `VarInt` from the start of `bytes`, returning `(value, bytes_read)`.
///
/// Enforces the 5-byte VarInt bound (`AGENTS.md` §4) and returns `None` on truncated or overlong input.
pub fn decode_mc_varint(bytes: &[u8]) -> Option<(u32, usize)> {
    let mut result = 0_u32;
    for (idx, &byte) in bytes.iter().take(5).enumerate() {
        let low = u32::from(byte & 0x7f);
        let shift = u32::try_from(idx).ok()?.saturating_mul(7);
        if idx == 4 && (byte & 0xf0) != 0 {
            return None;
        }
        result |= low.checked_shl(shift)?;
        if (byte & 0x80) == 0 {
            return Some((result, idx.saturating_add(1)));
        }
    }
    None
}

/// Build the Minecraft: Java Edition Server List Ping (SLP) Handshake (`0x00`, `next_state = 1`)
/// followed by the Status Request (`0x00`) packet (`T-090`, `GAME-002`).
pub fn build_mc_java_slp_handshake_and_status_request(
    protocol_version: u32,
    server_host: &str,
    server_port: u16,
) -> Vec<u8> {
    let bounded_host: String = server_host.chars().take(MAX_MC_SLP_HOST_BYTES).collect();
    let host_bytes = bounded_host.as_bytes();
    let host_len = u32::try_from(host_bytes.len()).unwrap_or(0);

    let mut handshake_body = Vec::with_capacity(16 + host_bytes.len());
    // Packet ID 0x00 (Handshake)
    handshake_body.extend_from_slice(&encode_mc_varint(0x00));
    // Protocol version VarInt
    handshake_body.extend_from_slice(&encode_mc_varint(protocol_version));
    // Server address string (VarInt byte length + UTF-8 bytes)
    handshake_body.extend_from_slice(&encode_mc_varint(host_len));
    handshake_body.extend_from_slice(host_bytes);
    // Server port (u16 big-endian)
    handshake_body.extend_from_slice(&server_port.to_be_bytes());
    // Next state = 1 (Status)
    handshake_body.extend_from_slice(&encode_mc_varint(1));

    let handshake_len = u32::try_from(handshake_body.len()).unwrap_or(0);
    let mut packet = encode_mc_varint(handshake_len);
    packet.extend_from_slice(&handshake_body);

    // Status Request packet: length = 1, packet_id = 0x00
    packet.push(0x01);
    packet.push(0x00);
    packet
}

/// Build a Minecraft: Java Edition Server List Ping (`0x01`) request frame carrying an 8-byte
/// big-endian timestamp/payload (`T-090`).
pub fn build_mc_java_slp_ping_request(payload: i64) -> [u8; 10] {
    let bytes = payload.to_be_bytes();
    [
        0x09, // Packet length = 9 bytes (1B packet ID + 8B payload)
        0x01, // Packet ID = 0x01 (Ping)
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]
}

/// Build a Minecraft: Java Edition Server List Ping Status Response (`0x00`) frame wrapping
/// `status_json` (`T-090`).
pub fn build_mc_java_slp_status_response(status_json: &str) -> Vec<u8> {
    let json_bytes = status_json.as_bytes();
    let json_len = u32::try_from(json_bytes.len()).unwrap_or(0);
    let mut body = Vec::with_capacity(6 + json_bytes.len());
    body.extend_from_slice(&encode_mc_varint(0x00));
    body.extend_from_slice(&encode_mc_varint(json_len));
    body.extend_from_slice(json_bytes);

    let total_len = u32::try_from(body.len()).unwrap_or(0);
    let mut frame = encode_mc_varint(total_len);
    frame.extend_from_slice(&body);
    frame
}

/// Parse a Minecraft: Java Edition Server List Ping Status Response (`0x00`) frame (`T-090`).
pub fn parse_mc_java_slp_status_response(
    frame: &[u8],
) -> Result<(MinecraftJavaStatusResponse, usize), String> {
    if frame.len() > MAX_MC_SLP_STATUS_BYTES {
        return Err(format!(
            "SLP frame length {} exceeds bound {MAX_MC_SLP_STATUS_BYTES}",
            frame.len()
        ));
    }
    let (packet_len, len_hdr_bytes) =
        decode_mc_varint(frame).ok_or_else(|| "invalid packet length VarInt".to_owned())?;
    let packet_len_usize =
        usize::try_from(packet_len).map_err(|_| "packet length overflow".to_owned())?;
    if packet_len_usize > MAX_MC_SLP_STATUS_BYTES {
        return Err(format!(
            "SLP declared packet length {packet_len_usize} exceeds bound {MAX_MC_SLP_STATUS_BYTES}"
        ));
    }
    let packet_end = len_hdr_bytes.saturating_add(packet_len_usize);
    let body = frame
        .get(len_hdr_bytes..packet_end)
        .ok_or_else(|| "truncated SLP status packet body".to_owned())?;

    let (packet_id, id_bytes) =
        decode_mc_varint(body).ok_or_else(|| "invalid SLP packet ID VarInt".to_owned())?;
    if packet_id != 0x00 {
        return Err(format!(
            "expected SLP Status Response packet ID 0x00, got 0x{packet_id:02x}"
        ));
    }

    let after_id = body
        .get(id_bytes..)
        .ok_or_else(|| "missing SLP string length".to_owned())?;
    let (str_len, str_hdr_bytes) =
        decode_mc_varint(after_id).ok_or_else(|| "invalid SLP string length VarInt".to_owned())?;
    let str_len_usize =
        usize::try_from(str_len).map_err(|_| "string length overflow".to_owned())?;
    let str_start = id_bytes.saturating_add(str_hdr_bytes);
    let str_end = str_start.saturating_add(str_len_usize);
    let json_slice = body
        .get(str_start..str_end)
        .ok_or_else(|| "truncated SLP JSON payload".to_owned())?;

    let parsed: MinecraftJavaStatusResponse = serde_json::from_slice(json_slice)
        .map_err(|e| format!("invalid SLP status JSON payload: {e}"))?;
    Ok((parsed, packet_end))
}

/// Execute an L1 Minecraft: Java Edition Server List Ping (SLP) probe against `target`
/// (`T-090`, `GAME-002`, `TEST-005`).
///
/// Connects over TCP, sends Handshake (`0x00`) + Status Request (`0x00`), parses the JSON Status
/// Response (`0x00`), sends Ping (`0x01`), and verifies the returned Pong (`0x01`) payload matches.
pub fn probe_minecraft_java_slp(
    target: SocketAddr,
    protocol_version: u32,
    ping_payload: i64,
    timeout: Duration,
) -> Result<MinecraftJavaSlpProbeReport, String> {
    let bounded_timeout = clamp_probe_timeout(timeout);
    let mut stream = TcpStream::connect_timeout(&target, bounded_timeout)
        .map_err(|e| format!("SLP TCP connect to {target} failed: {e}"))?;
    let _ = stream.set_read_timeout(Some(bounded_timeout));
    let _ = stream.set_write_timeout(Some(bounded_timeout));

    let req = build_mc_java_slp_handshake_and_status_request(
        protocol_version,
        &target.ip().to_string(),
        target.port(),
    );
    stream
        .write_all(&req)
        .map_err(|e| format!("SLP handshake write failed: {e}"))?;
    stream
        .flush()
        .map_err(|e| format!("SLP handshake flush failed: {e}"))?;

    let mut buf = Vec::with_capacity(512);
    let mut chunk = [0_u8; 1024];
    let status = loop {
        if buf.len() >= MAX_MC_SLP_STATUS_BYTES {
            return Err("SLP response exceeded maximum allowed byte bound".to_owned());
        }
        let n = stream
            .read(&mut chunk)
            .map_err(|e| format!("SLP status read failed: {e}"))?;
        if n == 0 {
            return Err("EOF while waiting for SLP status response".to_owned());
        }
        buf.extend_from_slice(chunk.get(..n).unwrap_or(&[]));
        if let Ok((parsed, consumed)) = parse_mc_java_slp_status_response(&buf) {
            let remaining = buf.get(consumed..).unwrap_or(&[]).to_vec();
            buf = remaining;
            break parsed;
        }
    };

    let ping_req = build_mc_java_slp_ping_request(ping_payload);
    stream
        .write_all(&ping_req)
        .map_err(|e| format!("SLP ping write failed: {e}"))?;
    stream
        .flush()
        .map_err(|e| format!("SLP ping flush failed: {e}"))?;

    while buf.len() < 10 {
        let n = stream
            .read(&mut chunk)
            .map_err(|e| format!("SLP pong read failed: {e}"))?;
        if n == 0 {
            return Err("EOF while waiting for SLP pong response".to_owned());
        }
        buf.extend_from_slice(chunk.get(..n).unwrap_or(&[]));
    }

    let expected_pong = build_mc_java_slp_ping_request(ping_payload);
    let pong_matched = buf.get(..10) == Some(expected_pong.as_slice());
    let _ = stream.shutdown(std::net::Shutdown::Both);

    if !pong_matched {
        return Err("SLP pong payload did not match sent ping payload".to_owned());
    }

    Ok(MinecraftJavaSlpProbeReport {
        probe: LocalProbeResult {
            mapping_key: "game".to_owned(),
            level: ProbeLevel::L1,
            status: ProbeStatus::Reachable,
            ok: true,
            detail: format!(
                "Minecraft Java SLP L1 ok: version={} (protocol {}), players={}/{}",
                status.version.name,
                status.version.protocol,
                status.players.online,
                status.players.max
            ),
        },
        status,
        pong_matched,
    })
}

fn clamp_probe_timeout(timeout: Duration) -> Duration {
    if timeout.is_zero() {
        DEFAULT_PROBE_TIMEOUT
    } else if timeout > Duration::from_secs(10) {
        Duration::from_secs(10)
    } else {
        timeout
    }
}
