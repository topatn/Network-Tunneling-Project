//! Relay session liveness (`ping`/`pong`), idle timeout detection, and grace-period resumption
//! (`T-063`, `contracts.md` §§10, 12, `NET-004`, `NET-005`, `FR-014`).
//!
//! ## Resumption semantics (`contracts.md` §10)
//!
//! * While a session is active, the relay tracks `last_peer_activity_ms` and replies to `ping(seq)`
//!   with `pong(seq)`.
//! * If the QUIC connection drops or `now_ms - last_peer_activity_ms >= idle_timeout_ms` (`NET-005`),
//!   the session enters [`RelaySessionConnectivity::DisconnectedInGrace`] until
//!   `dropped_at_ms + grace_period_ms`:
//!   * **Established TCP connections are closed immediately** (`RelayTcpSession::close_all_flows_on_session_drop`).
//!   * **Public port mappings and the UDP flow table (`RelayUdpSession`) are retained** during the
//!     grace window (`NET-004`, `FR-014`).
//! * If the engine reconnects with a valid allocation token before `grace_deadline_ms`, the session
//!   returns to [`RelaySessionConnectivity::Active`] with the same public ports and its retained
//!   UDP flows continue forwarding without losing guest `(mapping_index, guest_addr) <-> flow_id`
//!   bindings.
//! * If `now_ms >= grace_deadline_ms` before reconnect, the session enters
//!   [`RelaySessionConnectivity::Released`] and all reserved ports and UDP flows are released.

use gtt_protocol::{
    ControlMessage, FlowSummaryMessage, HelloAckMapping, HelloAckMessage, SequenceMessage,
    SessionLimits, DEFAULT_MAX_DATAGRAM_BYTES,
};

use crate::tcp::RelayTcpSession;
use crate::udp::{RelayUdpSession, DEFAULT_UDP_FLOW_TIMEOUT_MS};

/// Default relay resumption grace period (`30_000 ms`, *(proposal)*, `contracts.md` §10).
pub const DEFAULT_GRACE_PERIOD_MS: u64 = 30_000;

/// Connectivity state of a relay tunnel session across heartbeats, outages, and grace periods
/// (`T-063`, `NET-004`, `NET-005`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelaySessionConnectivity {
    /// QUIC control stream is live and within `idle_timeout_ms`.
    Active {
        /// Current QUIC session UUID (`hello_ack.session_id`).
        session_id: String,
    },
    /// Connection was lost or timed out; allocation, public ports, and UDP flows are retained
    /// until `grace_deadline_ms` while TCP streams have been closed (`contracts.md` §10).
    DisconnectedInGrace {
        /// Timestamp (ms) when the drop or idle timeout occurred.
        dropped_at_ms: u64,
        /// Timestamp (ms) when the grace period expires if the engine has not reconnected.
        grace_deadline_ms: u64,
    },
    /// Grace period elapsed or tunnel was explicitly stopped; ports and flows are released.
    Released,
}

/// Relay-side session coordinator combining `ping`/`pong` liveness, grace-period resumption,
/// [`RelayTcpSession`], and [`RelayUdpSession`] (`T-063`).
#[derive(Debug, Clone)]
pub struct RelayTunnelSession {
    allocation_id: String,
    tunnel_id: String,
    mappings: Vec<HelloAckMapping>,
    heartbeat_interval_ms: u32,
    idle_timeout_ms: u32,
    grace_period_ms: u64,
    limits: SessionLimits,
    connectivity: RelaySessionConnectivity,
    last_peer_activity_ms: u64,
    last_rtt_ms: Option<u64>,
    resumption_count: u32,
    tcp_closed_on_drop_total: u64,
    pub tcp: RelayTcpSession,
    pub udp: RelayUdpSession,
}

impl RelayTunnelSession {
    /// Construct a newly authenticated [`RelayTunnelSession`] at `now_ms`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        allocation_id: impl Into<String>,
        tunnel_id: impl Into<String>,
        session_id: impl Into<String>,
        mappings: Vec<HelloAckMapping>,
        heartbeat_interval_ms: u32,
        idle_timeout_ms: u32,
        grace_period_ms: u64,
        limits: SessionLimits,
        now_ms: u64,
    ) -> Self {
        let tcp = RelayTcpSession::new(&mappings, limits.max_tcp_flows);
        let udp = RelayUdpSession::new(
            &mappings,
            limits.max_udp_flows,
            DEFAULT_MAX_DATAGRAM_BYTES,
            DEFAULT_UDP_FLOW_TIMEOUT_MS,
        );
        Self {
            allocation_id: allocation_id.into(),
            tunnel_id: tunnel_id.into(),
            mappings,
            heartbeat_interval_ms,
            idle_timeout_ms,
            grace_period_ms: if grace_period_ms == 0 {
                DEFAULT_GRACE_PERIOD_MS
            } else {
                grace_period_ms
            },
            limits,
            connectivity: RelaySessionConnectivity::Active {
                session_id: session_id.into(),
            },
            last_peer_activity_ms: now_ms,
            last_rtt_ms: None,
            resumption_count: 0,
            tcp_closed_on_drop_total: 0,
            tcp,
            udp,
        }
    }

    /// Allocation UUID (`aid`).
    pub fn allocation_id(&self) -> &str {
        &self.allocation_id
    }

    /// Tunnel UUID (`sub`).
    pub fn tunnel_id(&self) -> &str {
        &self.tunnel_id
    }

    /// Borrow the reserved public port mappings (`contracts.md` §10).
    pub fn mappings(&self) -> &[HelloAckMapping] {
        &self.mappings
    }

    /// Current connectivity state (`Active`, `DisconnectedInGrace`, or `Released`).
    pub fn connectivity(&self) -> &RelaySessionConnectivity {
        &self.connectivity
    }

    /// Most recent sampled RTT in milliseconds from `ping`/`pong`.
    pub fn last_rtt_ms(&self) -> Option<u64> {
        self.last_rtt_ms
    }

    /// Number of times this session has successfully resumed within the grace window.
    pub fn resumption_count(&self) -> u32 {
        self.resumption_count
    }

    /// Total TCP flows closed due to tunnel connection drops or idle timeouts.
    pub fn tcp_closed_on_drop_total(&self) -> u64 {
        self.tcp_closed_on_drop_total
    }

    /// Record any inbound control or data activity from the engine at `now_ms`.
    pub fn record_peer_activity(&mut self, now_ms: u64) {
        self.last_peer_activity_ms = now_ms;
    }

    /// Handle an inbound `ping` from the engine and return the `pong` control message (`NET-005`).
    pub fn handle_ping(&mut self, ping: SequenceMessage, now_ms: u64) -> ControlMessage {
        self.last_peer_activity_ms = now_ms;
        ControlMessage::Pong(SequenceMessage { seq: ping.seq })
    }

    /// Handle an inbound `pong` from the engine for a `ping` sent at `sent_at_ms` (`NET-005`).
    pub fn handle_pong(&mut self, _pong: SequenceMessage, sent_at_ms: u64, now_ms: u64) -> u64 {
        self.last_peer_activity_ms = now_ms;
        let rtt = now_ms.saturating_sub(sent_at_ms);
        self.last_rtt_ms = Some(rtt);
        rtt
    }

    /// Explicitly notify the session that the underlying QUIC transport dropped at `now_ms`.
    ///
    /// Transitions `Active` -> `DisconnectedInGrace`, closes all established TCP flows, and
    /// retains the public port mappings and UDP flow table until `now_ms + grace_period_ms`
    /// (`contracts.md` §10, `NET-004`, `FR-014`).
    pub fn on_transport_disconnected(&mut self, now_ms: u64) -> &RelaySessionConnectivity {
        if matches!(self.connectivity, RelaySessionConnectivity::Active { .. }) {
            let closed = self.tcp.close_all_flows_on_session_drop();
            self.tcp_closed_on_drop_total = self
                .tcp_closed_on_drop_total
                .saturating_add(u64::from(closed));
            self.connectivity = RelaySessionConnectivity::DisconnectedInGrace {
                dropped_at_ms: now_ms,
                grace_deadline_ms: now_ms.saturating_add(self.grace_period_ms),
            };
        }
        &self.connectivity
    }

    /// Evaluate idle-timeout and grace-period deadlines at `now_ms` (`NET-004`, `NET-005`).
    pub fn check_liveness(&mut self, now_ms: u64) -> &RelaySessionConnectivity {
        match self.connectivity {
            RelaySessionConnectivity::Active { .. } => {
                if now_ms.saturating_sub(self.last_peer_activity_ms)
                    >= u64::from(self.idle_timeout_ms)
                {
                    let closed = self.tcp.close_all_flows_on_session_drop();
                    self.tcp_closed_on_drop_total = self
                        .tcp_closed_on_drop_total
                        .saturating_add(u64::from(closed));
                    self.connectivity = RelaySessionConnectivity::DisconnectedInGrace {
                        dropped_at_ms: now_ms,
                        grace_deadline_ms: now_ms.saturating_add(self.grace_period_ms),
                    };
                }
            }
            RelaySessionConnectivity::DisconnectedInGrace {
                grace_deadline_ms, ..
            } => {
                if now_ms >= grace_deadline_ms {
                    self.release();
                }
            }
            RelaySessionConnectivity::Released => {}
        }
        &self.connectivity
    }

    /// Resume a session on reconnect (`NET-004`, `FR-014`).
    ///
    /// If the session is in `DisconnectedInGrace` (or `Active` on an immediate superseding
    /// reconnect) and `now_ms < grace_deadline_ms`, transitions back to `Active` with the same
    /// reserved `mappings` (same public ports) and preserves the UDP flow table while ensuring any
    /// prior TCP flows are closed.
    pub fn resume(
        &mut self,
        new_session_id: impl Into<String>,
        now_ms: u64,
    ) -> Result<HelloAckMessage, &'static str> {
        self.check_liveness(now_ms);

        match self.connectivity {
            RelaySessionConnectivity::Released => {
                return Err("grace period expired; allocation has been released");
            }
            RelaySessionConnectivity::Active { .. } => {
                // Superseding an active connection closes its TCP streams while keeping UDP flows.
                let closed = self.tcp.close_all_flows_on_session_drop();
                self.tcp_closed_on_drop_total = self
                    .tcp_closed_on_drop_total
                    .saturating_add(u64::from(closed));
            }
            RelaySessionConnectivity::DisconnectedInGrace { .. } => {}
        }

        let sid = new_session_id.into();
        self.connectivity = RelaySessionConnectivity::Active {
            session_id: sid.clone(),
        };
        self.last_peer_activity_ms = now_ms;
        self.resumption_count = self.resumption_count.saturating_add(1);

        Ok(HelloAckMessage {
            session_id: sid,
            heartbeat_interval_ms: self.heartbeat_interval_ms,
            idle_timeout_ms: self.idle_timeout_ms,
            limits: self.limits,
            mappings: self.mappings.clone(),
        })
    }

    /// Release the session and all resources (`NET-008`).
    pub fn release(&mut self) {
        let closed = self.tcp.close_all_flows_on_session_drop();
        self.tcp_closed_on_drop_total = self
            .tcp_closed_on_drop_total
            .saturating_add(u64::from(closed));
        let _ = self.udp.clear_all_flows();
        self.connectivity = RelaySessionConnectivity::Released;
    }

    /// Build a `flow_summary` control message from current TCP and UDP flow state (`contracts.md` §10).
    pub fn build_flow_summary(&self) -> FlowSummaryMessage {
        FlowSummaryMessage {
            tcp_flows: u32::from(self.tcp.metrics().active_tcp_flows),
            udp_flows: u32::from(self.udp.metrics().active_udp_flows),
            distinct_sources: self
                .tcp
                .distinct_source_count()
                .saturating_add(self.udp.distinct_source_count()),
        }
    }
}
