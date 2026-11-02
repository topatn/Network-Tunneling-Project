//! Engine-side UDP forwarding path (`T-062`, `contracts.md` §11, `NET-002`, `NET-007`, `SEC-006`).
//!
//! ## Wire protocol and per-flow local sockets (`contracts.md` §11)
//!
//! * Every tunnel UDP datagram begins with a 6-byte header (`[mapping_index: u16, flow_id: u32]`
//!   big-endian) followed by the raw UDP payload.
//! * On the first datagram for a valid, enabled `Protocol::Udp` `mapping_index` in the engine's
//!   local mapping table (`SEC-006`), the engine binds **one dedicated local `UdpSocket` per
//!   `(mapping_index, flow_id)`** (`127.0.0.1:0` or `[::1]:0`) and connects it to the configured
//!   local game server target.
//! * Because each guest source is assigned a distinct `flow_id` by the relay and each `flow_id`
//!   has its own ephemeral local UDP socket on the host, the local game server sees distinct client
//!   ports for each guest and replies to the matching per-flow socket. The engine reads replies
//!   from each per-flow socket, wraps them with `(mapping_index, flow_id)`, and returns them to the
//!   relay (`NET-002`).
//! * Oversized packets (`> max_datagram_bytes`), truncated frames (`< 6` bytes), datagrams for
//!   unknown/disabled/non-UDP mapping indexes, and new flows exceeding `max_udp_flows` are dropped
//!   and counted in `datagrams_dropped` (`NET-007`).
//! * Idle flows are expired and their local UDP sockets closed (`contracts.md` §11).

use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use gtt_protocol::{
    encode_udp_datagram, FrameError, Protocol, UdpDatagram, DEFAULT_MAX_DATAGRAM_BYTES,
    UDP_HEADER_LEN,
};

use crate::tcp::LocalMappingConfig;

/// Default engine UDP flow idle timeout (`60_000 ms`, *(proposal)*).
pub const DEFAULT_ENGINE_UDP_FLOW_TIMEOUT_MS: u64 = 60_000;

/// Default non-blocking/short read timeout on per-flow local UDP sockets (`20 ms`).
pub const DEFAULT_LOCAL_UDP_SOCKET_TIMEOUT: Duration = Duration::from_millis(20);

/// Metrics tracked by the engine UDP forwarder (`contracts.md` §11, `NET-002`, `NET-007`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EngineUdpMetrics {
    /// Currently open per-flow local UDP sockets.
    pub active_udp_flows: u16,
    /// Total datagrams forwarded from the relay to local UDP targets.
    pub datagrams_to_local: u64,
    /// Total datagrams read from local UDP targets and framed for the relay.
    pub datagrams_from_local: u64,
    /// Total datagrams dropped across all drop reasons (`contracts.md` §11 required counter).
    pub datagrams_dropped: u64,
    /// Subset of `datagrams_dropped`: datagram exceeded `max_datagram_bytes`.
    pub dropped_oversize: u64,
    /// Subset of `datagrams_dropped`: `mapping_index` was unknown, disabled, or not `Protocol::Udp`.
    pub dropped_unknown_mapping: u64,
    /// Subset of `datagrams_dropped`: frame was shorter than 6 bytes (`UDP_HEADER_LEN`).
    pub dropped_malformed: u64,
    /// Subset of `datagrams_dropped`: new flow refused because `max_udp_flows` was reached or flow_id == 0.
    pub dropped_flow_limit: u64,
    /// Subset of `datagrams_dropped`: local socket bind/send failure.
    pub dropped_io_error: u64,
    /// Total idle UDP flows expired and closed.
    pub flows_expired: u64,
}

/// Reason an inbound tunnel datagram was dropped by [`EngineUdpSession::handle_relay_datagram`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineUdpDropReason {
    /// Frame was shorter than 6 bytes (`UDP_HEADER_LEN`).
    MalformedHeader,
    /// Frame exceeded `max_datagram_bytes`.
    OversizedDatagram,
    /// `mapping_index` does not exist, is disabled, or is not `Protocol::Udp`.
    UnknownOrDisabledMapping,
    /// `flow_id == 0` or `active_udp_flows >= max_udp_flows`.
    FlowLimitReached,
    /// Local UDP socket bind/connect/send failed.
    SocketIoError,
}

#[derive(Debug)]
struct EngineUdpFlow {
    mapping_index: u16,
    flow_id: u32,
    socket: UdpSocket,
    last_active_ms: u64,
}

/// Engine-side per-session UDP forwarder managing per-flow local `UdpSocket`s (`T-062`, `NET-002`,
/// `NET-007`, `SEC-006`).
#[derive(Debug)]
pub struct EngineUdpSession {
    udp_mappings: HashMap<u16, LocalMappingConfig>,
    max_udp_flows: u16,
    max_datagram_bytes: usize,
    idle_flow_timeout_ms: u64,
    flows: HashMap<(u16, u32), EngineUdpFlow>,
    flow_order: Vec<(u16, u32)>,
    metrics: EngineUdpMetrics,
}

impl EngineUdpSession {
    /// Construct a validated [`EngineUdpSession`] from the host's local mapping configurations.
    pub fn new(
        configs: Vec<LocalMappingConfig>,
        max_udp_flows: u16,
        max_datagram_bytes: usize,
        idle_flow_timeout_ms: u64,
    ) -> Result<Self, &'static str> {
        let mut udp_mappings = HashMap::new();
        for cfg in configs {
            cfg.validate()?;
            if cfg.enabled
                && cfg.protocol == Protocol::Udp
                && udp_mappings.insert(cfg.index, cfg).is_some()
            {
                return Err("duplicate UDP mapping index in LocalMappingConfig list");
            }
        }
        let clamped_datagram = if max_datagram_bytes < UDP_HEADER_LEN {
            DEFAULT_MAX_DATAGRAM_BYTES
        } else {
            max_datagram_bytes
        };
        let clamped_timeout = if idle_flow_timeout_ms == 0 {
            DEFAULT_ENGINE_UDP_FLOW_TIMEOUT_MS
        } else {
            idle_flow_timeout_ms
        };
        Ok(Self {
            udp_mappings,
            max_udp_flows: max_udp_flows.clamp(1, 4096),
            max_datagram_bytes: clamped_datagram,
            idle_flow_timeout_ms: clamped_timeout,
            flows: HashMap::new(),
            flow_order: Vec::new(),
            metrics: EngineUdpMetrics::default(),
        })
    }

    /// Borrow current UDP forwarding metrics.
    pub fn metrics(&self) -> &EngineUdpMetrics {
        &self.metrics
    }

    /// Configured maximum tunnel datagram size (6-byte header + payload).
    pub fn max_datagram_bytes(&self) -> usize {
        self.max_datagram_bytes
    }

    /// Maximum usable UDP payload size (`max_datagram_bytes - 6`).
    pub fn max_payload_bytes(&self) -> usize {
        self.max_datagram_bytes.saturating_sub(UDP_HEADER_LEN)
    }

    /// Return the local ephemeral socket address bound for `(mapping_index, flow_id)`, if active.
    pub fn flow_local_addr(&self, mapping_index: u16, flow_id: u32) -> Option<SocketAddr> {
        self.flows
            .get(&(mapping_index, flow_id))
            .and_then(|f| f.socket.local_addr().ok())
    }

    /// Expire and close all per-flow local UDP sockets idle for `>= idle_flow_timeout_ms`.
    pub fn expire_idle_flows(&mut self, now_ms: u64) -> usize {
        let timeout = self.idle_flow_timeout_ms;
        let mut expired = Vec::new();
        for (&key, flow) in &self.flows {
            if now_ms.saturating_sub(flow.last_active_ms) >= timeout {
                expired.push(key);
            }
        }
        let count = expired.len();
        for key in expired {
            self.flows.remove(&key);
            self.flow_order.retain(|&k| k != key);
            self.metrics.flows_expired = self.metrics.flows_expired.saturating_add(1);
        }
        self.metrics.active_udp_flows = u16::try_from(self.flows.len()).unwrap_or(u16::MAX);
        count
    }

    /// Handle a framed QUIC DATAGRAM from the relay and forward its payload to the local game
    /// server through the per-flow local `UdpSocket` (`contracts.md` §11, `NET-002`, `NET-007`).
    pub fn handle_relay_datagram(
        &mut self,
        raw_frame: &[u8],
        now_ms: u64,
    ) -> Result<(), EngineUdpDropReason> {
        self.expire_idle_flows(now_ms);

        let decoded = match UdpDatagram::decode(raw_frame, self.max_datagram_bytes) {
            Ok(d) => d,
            Err(FrameError::OversizedDatagram { .. }) => {
                self.record_drop(EngineUdpDropReason::OversizedDatagram);
                return Err(EngineUdpDropReason::OversizedDatagram);
            }
            Err(_) => {
                self.record_drop(EngineUdpDropReason::MalformedHeader);
                return Err(EngineUdpDropReason::MalformedHeader);
            }
        };

        let Some(cfg) = self.udp_mappings.get(&decoded.mapping_index).cloned() else {
            self.record_drop(EngineUdpDropReason::UnknownOrDisabledMapping);
            return Err(EngineUdpDropReason::UnknownOrDisabledMapping);
        };

        if decoded.flow_id == 0 {
            self.record_drop(EngineUdpDropReason::FlowLimitReached);
            return Err(EngineUdpDropReason::FlowLimitReached);
        }

        let key = (decoded.mapping_index, decoded.flow_id);
        if !self.flows.contains_key(&key) {
            if self.flows.len() >= usize::from(self.max_udp_flows) {
                self.record_drop(EngineUdpDropReason::FlowLimitReached);
                return Err(EngineUdpDropReason::FlowLimitReached);
            }
            let socket = match bind_connected_flow_socket(cfg.local_addr) {
                Ok(s) => s,
                Err(_) => {
                    self.record_drop(EngineUdpDropReason::SocketIoError);
                    return Err(EngineUdpDropReason::SocketIoError);
                }
            };
            self.flows.insert(
                key,
                EngineUdpFlow {
                    mapping_index: decoded.mapping_index,
                    flow_id: decoded.flow_id,
                    socket,
                    last_active_ms: now_ms,
                },
            );
            self.flow_order.push(key);
            self.metrics.active_udp_flows = u16::try_from(self.flows.len()).unwrap_or(u16::MAX);
        }

        let send_res = if let Some(flow) = self.flows.get_mut(&key) {
            flow.last_active_ms = now_ms;
            flow.socket.send(decoded.payload)
        } else {
            Err(io::Error::other("missing flow socket"))
        };

        match send_res {
            Ok(_) => {
                self.metrics.datagrams_to_local = self.metrics.datagrams_to_local.saturating_add(1);
                Ok(())
            }
            Err(_) => {
                self.record_drop(EngineUdpDropReason::SocketIoError);
                Err(EngineUdpDropReason::SocketIoError)
            }
        }
    }

    /// Poll all active per-flow local `UdpSocket`s for replies from the local game server, framing
    /// each reply with its `(mapping_index, flow_id)` 6-byte header (`contracts.md` §11, `NET-002`).
    ///
    /// At most `max_packets_per_flow` packets are drained per flow per call so a noisy flow cannot
    /// starve other flows (`AGENTS.md` §4).
    pub fn poll_local_replies(&mut self, max_packets_per_flow: usize, now_ms: u64) -> Vec<Vec<u8>> {
        self.expire_idle_flows(now_ms);

        let per_flow_cap = max_packets_per_flow.clamp(1, 64);
        // Buffer size is max_datagram_bytes + 1 so we can detect an oversized reply from the local server.
        let recv_buf_len = self
            .max_datagram_bytes
            .saturating_sub(UDP_HEADER_LEN)
            .saturating_add(2)
            .max(2048);
        let mut recv_buf = vec![0u8; recv_buf_len];
        let mut out_frames = Vec::new();
        let mut oversize_drops = 0u64;

        let keys_snapshot = self.flow_order.clone();
        for key in keys_snapshot {
            let Some(flow) = self.flows.get_mut(&key) else {
                continue;
            };
            for _ in 0..per_flow_cap {
                match flow.socket.recv(&mut recv_buf) {
                    Ok(n) => {
                        let payload = recv_buf.get(..n).unwrap_or(&[]);
                        match encode_udp_datagram(
                            flow.mapping_index,
                            flow.flow_id,
                            payload,
                            self.max_datagram_bytes,
                        ) {
                            Ok(framed) => {
                                flow.last_active_ms = now_ms;
                                self.metrics.datagrams_from_local =
                                    self.metrics.datagrams_from_local.saturating_add(1);
                                out_frames.push(framed);
                            }
                            Err(_) => {
                                oversize_drops = oversize_drops.saturating_add(1);
                            }
                        }
                    }
                    Err(ref err)
                        if err.kind() == io::ErrorKind::WouldBlock
                            || err.kind() == io::ErrorKind::TimedOut =>
                    {
                        break;
                    }
                    Err(_) => break,
                }
            }
        }

        if oversize_drops > 0 {
            self.metrics.datagrams_dropped = self
                .metrics
                .datagrams_dropped
                .saturating_add(oversize_drops);
            self.metrics.dropped_oversize =
                self.metrics.dropped_oversize.saturating_add(oversize_drops);
        }

        out_frames
    }

    /// Close all per-flow local UDP sockets (`NET-008` shutdown cleanup).
    pub fn close_all_flows(&mut self) -> usize {
        let count = self.flows.len();
        self.flows.clear();
        self.flow_order.clear();
        self.metrics.active_udp_flows = 0;
        count
    }

    fn record_drop(&mut self, reason: EngineUdpDropReason) {
        self.metrics.datagrams_dropped = self.metrics.datagrams_dropped.saturating_add(1);
        match reason {
            EngineUdpDropReason::OversizedDatagram => {
                self.metrics.dropped_oversize = self.metrics.dropped_oversize.saturating_add(1);
            }
            EngineUdpDropReason::UnknownOrDisabledMapping => {
                self.metrics.dropped_unknown_mapping =
                    self.metrics.dropped_unknown_mapping.saturating_add(1);
            }
            EngineUdpDropReason::MalformedHeader => {
                self.metrics.dropped_malformed = self.metrics.dropped_malformed.saturating_add(1);
            }
            EngineUdpDropReason::FlowLimitReached => {
                self.metrics.dropped_flow_limit = self.metrics.dropped_flow_limit.saturating_add(1);
            }
            EngineUdpDropReason::SocketIoError => {
                self.metrics.dropped_io_error = self.metrics.dropped_io_error.saturating_add(1);
            }
        }
    }
}

fn bind_connected_flow_socket(target: SocketAddr) -> io::Result<UdpSocket> {
    let bind_addr = match target {
        SocketAddr::V4(_) => SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0),
        SocketAddr::V6(_) => SocketAddr::new(Ipv6Addr::LOCALHOST.into(), 0),
    };
    let socket = UdpSocket::bind(bind_addr)?;
    socket.set_read_timeout(Some(DEFAULT_LOCAL_UDP_SOCKET_TIMEOUT))?;
    socket.set_write_timeout(Some(DEFAULT_LOCAL_UDP_SOCKET_TIMEOUT))?;
    socket.connect(target)?;
    Ok(socket)
}
