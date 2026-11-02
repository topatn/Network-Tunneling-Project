//! Relay-side UDP forwarding path (`T-062`, `contracts.md` §11, `NET-002`, `NET-007`, `SEC-005`).
//!
//! ## Wire protocol (`contracts.md` §11)
//!
//! Each UDP datagram carried over the tunnel has a 6-byte header (`[mapping_index: u16, flow_id: u32]`
//! big-endian) followed by the raw UDP payload:
//!
//! * **Guest -> Relay -> Engine:**
//!   1. Guest sends a UDP datagram to the relay's reserved public UDP socket for `mapping_index`.
//!   2. If `payload.len() + 6 > max_datagram_bytes`, the packet is dropped and `datagrams_dropped`
//!      (`dropped_oversize`) is incremented (`NET-007`).
//!   3. If `mapping_index` is not an authorized UDP mapping in `hello_ack.mappings`, the packet is
//!      dropped and `datagrams_dropped` (`dropped_unknown_mapping`) is incremented (`NET-007`).
//!   4. The relay looks up or assigns a `flow_id` (`u32`, `>= 1`) for `(mapping_index, guest_addr)`,
//!      enforcing `max_udp_flows` after evicting expired flows (`idle_flow_timeout_ms`).
//!   5. The relay prepends `(mapping_index, flow_id)` and forwards the framed datagram to the
//!      engine.
//! * **Engine -> Relay -> Guest:**
//!   1. The relay decodes `(mapping_index, flow_id, payload)` via [`UdpDatagram::decode`],
//!      enforcing `max_datagram_bytes` and minimum 6-byte header length (`NET-007`).
//!   2. The relay looks up `(mapping_index, flow_id)` in its flow table, refreshes `last_active_ms`,
//!      and sends `payload` back to the exact `guest_addr` so two guest sources never receive each
//!      other's replies (`NET-002`).

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};

use gtt_protocol::{
    encode_udp_datagram, FrameError, HelloAckMapping, Protocol, UdpDatagram,
    DEFAULT_MAX_DATAGRAM_BYTES, UDP_HEADER_LEN,
};

/// Default UDP flow idle expiration timeout (`60_000 ms`, *(proposal)*).
pub const DEFAULT_UDP_FLOW_TIMEOUT_MS: u64 = 60_000;

/// Counters tracked by the relay UDP forwarding path (`contracts.md` §11, `NET-002`, `NET-007`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RelayUdpMetrics {
    /// Currently active UDP flows in the table.
    pub active_udp_flows: u16,
    /// Total datagrams forwarded from guests to the engine.
    pub datagrams_guest_to_engine: u64,
    /// Total datagrams forwarded from the engine to guests.
    pub datagrams_engine_to_guest: u64,
    /// Total datagrams dropped across all drop reasons (`contracts.md` §11 required counter).
    pub datagrams_dropped: u64,
    /// Subset of `datagrams_dropped`: datagram exceeded `max_datagram_bytes`.
    pub dropped_oversize: u64,
    /// Subset of `datagrams_dropped`: `mapping_index` was not an active UDP mapping.
    pub dropped_unknown_mapping: u64,
    /// Subset of `datagrams_dropped`: frame was shorter than 6 bytes (`UDP_HEADER_LEN`).
    pub dropped_malformed: u64,
    /// Subset of `datagrams_dropped`: new guest source refused because `max_udp_flows` was reached.
    pub dropped_flow_limit: u64,
    /// Subset of `datagrams_dropped`: engine sent a reply for an unknown or expired `flow_id`.
    pub dropped_unknown_flow: u64,
    /// Total UDP flows evicted after exceeding `idle_flow_timeout_ms`.
    pub flows_expired: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RelayUdpFlowEntry {
    mapping_index: u16,
    flow_id: u32,
    guest_addr: SocketAddr,
    last_active_ms: u64,
}

/// Relay-side per-session UDP flow table and datagram framer (`T-062`, `NET-002`, `NET-007`).
#[derive(Debug, Clone)]
pub struct RelayUdpSession {
    udp_mapping_indexes: HashSet<u16>,
    max_udp_flows: u16,
    max_datagram_bytes: usize,
    idle_flow_timeout_ms: u64,
    next_flow_id: u32,
    by_source: HashMap<(u16, SocketAddr), u32>,
    by_flow_id: HashMap<(u16, u32), RelayUdpFlowEntry>,
    distinct_sources: HashSet<IpAddr>,
    max_distinct_sources: usize,
    metrics: RelayUdpMetrics,
}

impl RelayUdpSession {
    /// Construct a [`RelayUdpSession`] for the given `hello_ack.mappings` and limits.
    pub fn new(
        mappings: &[HelloAckMapping],
        max_udp_flows: u16,
        max_datagram_bytes: usize,
        idle_flow_timeout_ms: u64,
    ) -> Self {
        let udp_mapping_indexes = mappings
            .iter()
            .filter(|m| m.protocol == Protocol::Udp)
            .map(|m| m.index)
            .collect();
        let clamped_datagram = if max_datagram_bytes < UDP_HEADER_LEN {
            DEFAULT_MAX_DATAGRAM_BYTES
        } else {
            max_datagram_bytes
        };
        let clamped_timeout = if idle_flow_timeout_ms == 0 {
            DEFAULT_UDP_FLOW_TIMEOUT_MS
        } else {
            idle_flow_timeout_ms
        };
        Self {
            udp_mapping_indexes,
            max_udp_flows: max_udp_flows.clamp(1, 4096),
            max_datagram_bytes: clamped_datagram,
            idle_flow_timeout_ms: clamped_timeout,
            next_flow_id: 1,
            by_source: HashMap::new(),
            by_flow_id: HashMap::new(),
            distinct_sources: HashSet::new(),
            max_distinct_sources: 4096,
            metrics: RelayUdpMetrics::default(),
        }
    }

    /// Borrow current UDP forwarding metrics (`datagrams_dropped`, active flows, etc.).
    pub fn metrics(&self) -> &RelayUdpMetrics {
        &self.metrics
    }

    /// Configured maximum QUIC datagram size (including the 6-byte header).
    pub fn max_datagram_bytes(&self) -> usize {
        self.max_datagram_bytes
    }

    /// Maximum usable UDP payload size (`max_datagram_bytes - 6`).
    pub fn max_payload_bytes(&self) -> usize {
        self.max_datagram_bytes.saturating_sub(UDP_HEADER_LEN)
    }

    /// Count of distinct guest source IP addresses observed (`contracts.md` §10 `flow_summary`).
    pub fn distinct_source_count(&self) -> u32 {
        u32::try_from(self.distinct_sources.len()).unwrap_or(u32::MAX)
    }

    /// Lookup the assigned `flow_id` for `(mapping_index, guest_addr)`, if active.
    pub fn flow_id_for_source(&self, mapping_index: u16, guest_addr: SocketAddr) -> Option<u32> {
        self.by_source.get(&(mapping_index, guest_addr)).copied()
    }

    /// Evict all UDP flows that have been idle for at least `idle_flow_timeout_ms`.
    pub fn expire_idle_flows(&mut self, now_ms: u64) -> usize {
        let timeout = self.idle_flow_timeout_ms;
        let mut expired_keys = Vec::new();
        for (&key, entry) in &self.by_flow_id {
            if now_ms.saturating_sub(entry.last_active_ms) >= timeout {
                expired_keys.push((key, entry.guest_addr));
            }
        }
        let count = expired_keys.len();
        for ((mapping_index, flow_id), guest_addr) in expired_keys {
            self.by_flow_id.remove(&(mapping_index, flow_id));
            self.by_source.remove(&(mapping_index, guest_addr));
            self.metrics.flows_expired = self.metrics.flows_expired.saturating_add(1);
        }
        self.metrics.active_udp_flows = u16::try_from(self.by_flow_id.len()).unwrap_or(u16::MAX);
        count
    }

    /// Clear all UDP flow table entries on session release or shutdown (`NET-008`).
    pub fn clear_all_flows(&mut self) -> u16 {
        let cleared = self.metrics.active_udp_flows;
        self.by_flow_id.clear();
        self.by_source.clear();
        self.distinct_sources.clear();
        self.metrics.active_udp_flows = 0;
        cleared
    }

    /// Handle an inbound UDP packet from `guest_addr` on public listener `mapping_index` and
    /// return the 6-byte framed tunnel datagram to send to the engine (`contracts.md` §11).
    ///
    /// Returns `None` if the packet was dropped (oversized, unknown mapping index, or flow limit
    /// reached), incrementing `metrics.datagrams_dropped` (`NET-007`).
    pub fn handle_guest_datagram(
        &mut self,
        mapping_index: u16,
        guest_addr: SocketAddr,
        payload: &[u8],
        now_ms: u64,
    ) -> Option<Vec<u8>> {
        self.expire_idle_flows(now_ms);

        // 1. Validate mapping_index first so unknown-index packets are dropped and counted.
        if !self.udp_mapping_indexes.contains(&mapping_index) {
            self.record_drop_unknown_mapping();
            return None;
        }

        // 2. Validate size bound (`payload.len() + 6 <= max_datagram_bytes`) before allocating a flow.
        if UDP_HEADER_LEN.saturating_add(payload.len()) > self.max_datagram_bytes {
            self.record_drop_oversize();
            return None;
        }

        // 3. Look up existing flow or allocate a new one within `max_udp_flows`.
        let flow_id = if let Some(&existing_id) = self.by_source.get(&(mapping_index, guest_addr)) {
            if let Some(entry) = self.by_flow_id.get_mut(&(mapping_index, existing_id)) {
                entry.last_active_ms = now_ms;
            }
            existing_id
        } else {
            if self.by_flow_id.len() >= usize::from(self.max_udp_flows) {
                self.record_drop_flow_limit();
                return None;
            }
            let assigned = self.allocate_flow_id(mapping_index);
            let entry = RelayUdpFlowEntry {
                mapping_index,
                flow_id: assigned,
                guest_addr,
                last_active_ms: now_ms,
            };
            self.by_source.insert((mapping_index, guest_addr), assigned);
            self.by_flow_id.insert((mapping_index, assigned), entry);
            self.metrics.active_udp_flows =
                u16::try_from(self.by_flow_id.len()).unwrap_or(u16::MAX);
            if self.distinct_sources.len() < self.max_distinct_sources {
                self.distinct_sources.insert(guest_addr.ip());
            }
            assigned
        };

        match encode_udp_datagram(mapping_index, flow_id, payload, self.max_datagram_bytes) {
            Ok(framed) => {
                self.metrics.datagrams_guest_to_engine =
                    self.metrics.datagrams_guest_to_engine.saturating_add(1);
                Some(framed)
            }
            Err(FrameError::OversizedDatagram { .. }) => {
                self.record_drop_oversize();
                None
            }
            Err(_) => {
                self.record_drop_malformed();
                None
            }
        }
    }

    /// Handle an inbound QUIC DATAGRAM frame from the host engine and return
    /// `(mapping_index, guest_addr, payload)` to transmit out the relay's public UDP socket
    /// (`contracts.md` §11, `NET-002`, `NET-007`).
    ///
    /// Returns `None` if the frame is truncated, oversized, references an unknown UDP mapping
    /// index, or references an unknown/expired `flow_id`, incrementing `datagrams_dropped`.
    pub fn handle_engine_datagram(
        &mut self,
        raw_frame: &[u8],
        now_ms: u64,
    ) -> Option<(u16, SocketAddr, Vec<u8>)> {
        self.expire_idle_flows(now_ms);

        let decoded = match UdpDatagram::decode(raw_frame, self.max_datagram_bytes) {
            Ok(d) => d,
            Err(FrameError::OversizedDatagram { .. }) => {
                self.record_drop_oversize();
                return None;
            }
            Err(
                FrameError::TruncatedUdpHeader { .. } | FrameError::InvalidDatagramLimit { .. },
            ) => {
                self.record_drop_malformed();
                return None;
            }
            Err(_) => {
                self.record_drop_malformed();
                return None;
            }
        };

        if !self.udp_mapping_indexes.contains(&decoded.mapping_index) {
            self.record_drop_unknown_mapping();
            return None;
        }

        let Some(entry) = self
            .by_flow_id
            .get_mut(&(decoded.mapping_index, decoded.flow_id))
        else {
            self.record_drop_unknown_flow();
            return None;
        };

        entry.last_active_ms = now_ms;
        self.metrics.datagrams_engine_to_guest =
            self.metrics.datagrams_engine_to_guest.saturating_add(1);
        Some((
            decoded.mapping_index,
            entry.guest_addr,
            decoded.payload.to_vec(),
        ))
    }

    fn allocate_flow_id(&mut self, mapping_index: u16) -> u32 {
        // Bounded search (at most max_udp_flows + 1 steps) so there is never an unbounded loop.
        let limit = u32::from(self.max_udp_flows).saturating_add(2);
        for _ in 0..limit {
            let candidate = if self.next_flow_id == 0 {
                1
            } else {
                self.next_flow_id
            };
            self.next_flow_id = candidate.wrapping_add(1);
            if self.next_flow_id == 0 {
                self.next_flow_id = 1;
            }
            if !self.by_flow_id.contains_key(&(mapping_index, candidate)) {
                return candidate;
            }
        }
        1
    }

    fn record_drop_oversize(&mut self) {
        self.metrics.datagrams_dropped = self.metrics.datagrams_dropped.saturating_add(1);
        self.metrics.dropped_oversize = self.metrics.dropped_oversize.saturating_add(1);
    }

    fn record_drop_unknown_mapping(&mut self) {
        self.metrics.datagrams_dropped = self.metrics.datagrams_dropped.saturating_add(1);
        self.metrics.dropped_unknown_mapping =
            self.metrics.dropped_unknown_mapping.saturating_add(1);
    }

    fn record_drop_malformed(&mut self) {
        self.metrics.datagrams_dropped = self.metrics.datagrams_dropped.saturating_add(1);
        self.metrics.dropped_malformed = self.metrics.dropped_malformed.saturating_add(1);
    }

    fn record_drop_flow_limit(&mut self) {
        self.metrics.datagrams_dropped = self.metrics.datagrams_dropped.saturating_add(1);
        self.metrics.dropped_flow_limit = self.metrics.dropped_flow_limit.saturating_add(1);
    }

    fn record_drop_unknown_flow(&mut self) {
        self.metrics.datagrams_dropped = self.metrics.datagrams_dropped.saturating_add(1);
        self.metrics.dropped_unknown_flow = self.metrics.dropped_unknown_flow.saturating_add(1);
    }
}
