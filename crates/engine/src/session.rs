//! Engine session heartbeat (`ping`/`pong`), idle-timeout detection, and automatic reconnection
//! with TCP closure and UDP continuity (`T-063`, `contracts.md` §§10, 12, `NET-004`, `NET-005`,
//! `FR-014`).

use std::collections::HashMap;

use gtt_protocol::{
    ControlMessage, EngineSessionEvent, EngineSessionState, HelloAckMapping, HelloAckMessage,
    InvalidSessionTransition, RejectCode, SequenceMessage, DEFAULT_MAX_DATAGRAM_BYTES,
};

use crate::tcp::{EngineTcpSession, LocalMappingConfig};
use crate::udp::{EngineUdpSession, DEFAULT_ENGINE_UDP_FLOW_TIMEOUT_MS};

/// Default RTT threshold (ms) above which an `Active` session transitions to `Degraded` (`250 ms`).
pub const DEFAULT_DEGRADED_RTT_THRESHOLD_MS: u64 = 250;

/// Default maximum reconnect attempts before transitioning `Reconnecting -> Failed` (`5`).
pub const DEFAULT_MAX_RECONNECT_ATTEMPTS: u32 = 5;

/// Engine-side session controller coordinating [`EngineSessionState`], `ping`/`pong` heartbeats,
/// idle-timeout detection, [`EngineTcpSession`] closure on drop, and [`EngineUdpSession`]
/// continuity across reconnects (`T-063`, `NET-004`, `NET-005`, `FR-014`).
#[derive(Debug)]
pub struct EngineTunnelSession {
    state: EngineSessionState,
    session_id: Option<String>,
    heartbeat_interval_ms: u32,
    idle_timeout_ms: u32,
    degraded_rtt_threshold_ms: u64,
    max_reconnect_attempts: u32,
    reconnect_attempts: u32,
    next_ping_seq: u64,
    in_flight_pings: HashMap<u64, u64>,
    last_peer_activity_ms: u64,
    last_rtt_ms: Option<u64>,
    negotiated_mappings: Vec<HelloAckMapping>,
    pub tcp: EngineTcpSession,
    pub udp: EngineUdpSession,
}

impl EngineTunnelSession {
    /// Construct a new [`EngineTunnelSession`] in [`EngineSessionState::Idle`].
    pub fn new(
        local_mappings: Vec<LocalMappingConfig>,
        max_tcp_flows: u16,
        max_udp_flows: u16,
    ) -> Result<Self, &'static str> {
        let tcp = EngineTcpSession::new(local_mappings.clone(), max_tcp_flows)?;
        let udp = EngineUdpSession::new(
            local_mappings,
            max_udp_flows,
            DEFAULT_MAX_DATAGRAM_BYTES,
            DEFAULT_ENGINE_UDP_FLOW_TIMEOUT_MS,
        )?;
        Ok(Self {
            state: EngineSessionState::Idle,
            session_id: None,
            heartbeat_interval_ms: 5_000,
            idle_timeout_ms: 15_000,
            degraded_rtt_threshold_ms: DEFAULT_DEGRADED_RTT_THRESHOLD_MS,
            max_reconnect_attempts: DEFAULT_MAX_RECONNECT_ATTEMPTS,
            reconnect_attempts: 0,
            next_ping_seq: 1,
            in_flight_pings: HashMap::new(),
            last_peer_activity_ms: 0,
            last_rtt_ms: None,
            negotiated_mappings: Vec::new(),
            tcp,
            udp,
        })
    }

    /// Current engine session state (`contracts.md` §12).
    pub fn state(&self) -> EngineSessionState {
        self.state
    }

    /// Currently active relay `session_id` from the latest `hello_ack`.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Public port mappings negotiated in `hello_ack`.
    pub fn negotiated_mappings(&self) -> &[HelloAckMapping] {
        &self.negotiated_mappings
    }

    /// Most recent sampled `ping`/`pong` RTT in milliseconds.
    pub fn last_rtt_ms(&self) -> Option<u64> {
        self.last_rtt_ms
    }

    /// Begin starting the tunnel (`Idle -> Connecting`).
    pub fn start(&mut self) -> Result<EngineSessionState, InvalidSessionTransition> {
        self.state = self.state.transition(EngineSessionEvent::StartWithToken)?;
        Ok(self.state)
    }

    /// Record that the outbound QUIC connection to the relay completed TLS handshake
    /// (`Connecting -> Authenticating`).
    pub fn on_quic_established(&mut self) -> Result<EngineSessionState, InvalidSessionTransition> {
        self.state = self.state.transition(EngineSessionEvent::QuicEstablished)?;
        Ok(self.state)
    }

    /// Process a `hello_ack` from the relay on initial connect (`Authenticating -> Active`) or
    /// during automatic recovery (`Reconnecting -> Active`, `FR-014`, `NET-004`).
    pub fn on_hello_ack(
        &mut self,
        ack: HelloAckMessage,
        now_ms: u64,
    ) -> Result<EngineSessionState, InvalidSessionTransition> {
        let event = if self.state == EngineSessionState::Reconnecting {
            EngineSessionEvent::ReconnectSucceeded
        } else {
            EngineSessionEvent::HelloAckReceived
        };
        self.state = self.state.transition(event)?;
        self.session_id = Some(ack.session_id);
        self.heartbeat_interval_ms = ack.heartbeat_interval_ms;
        self.idle_timeout_ms = ack.idle_timeout_ms;
        self.negotiated_mappings = ack.mappings;
        self.reconnect_attempts = 0;
        self.in_flight_pings.clear();
        self.last_peer_activity_ms = now_ms;
        Ok(self.state)
    }

    /// Process a `reject` message from the relay (`contracts.md` §§10, 12).
    pub fn on_reject(
        &mut self,
        code: RejectCode,
    ) -> Result<EngineSessionState, InvalidSessionTransition> {
        if self.state == EngineSessionState::Reconnecting {
            if !code.is_retryable() {
                self.state = self
                    .state
                    .transition(EngineSessionEvent::RetriesExhausted)?;
                self.udp.close_all_flows();
                return Ok(self.state);
            }
            self.reconnect_attempts = self.reconnect_attempts.saturating_add(1);
            if self.reconnect_attempts >= self.max_reconnect_attempts {
                self.state = self
                    .state
                    .transition(EngineSessionEvent::RetriesExhausted)?;
                self.udp.close_all_flows();
            }
            return Ok(self.state);
        }
        self.state = self.state.transition(EngineSessionEvent::Rejected(code))?;
        if self.state == EngineSessionState::Failed {
            self.udp.close_all_flows();
        }
        Ok(self.state)
    }

    /// Emit the next outbound `ping` control message at `now_ms` (`NET-005`).
    pub fn build_ping(&mut self, now_ms: u64) -> ControlMessage {
        let seq = self.next_ping_seq;
        self.next_ping_seq = self.next_ping_seq.saturating_add(1);
        // Bound in_flight_pings so unacknowledged pings cannot grow without bound (`AGENTS.md` §4).
        if self.in_flight_pings.len() >= 64 {
            if let Some(&oldest_seq) = self.in_flight_pings.keys().min() {
                self.in_flight_pings.remove(&oldest_seq);
            }
        }
        self.in_flight_pings.insert(seq, now_ms);
        ControlMessage::Ping(SequenceMessage { seq })
    }

    /// Handle an inbound `pong` from the relay (`NET-005`).
    ///
    /// Updates `last_peer_activity_ms`, computes RTT if `seq` was in flight, and transitions
    /// between `Active` and `Degraded` based on `degraded_rtt_threshold_ms`.
    pub fn on_pong(&mut self, pong: SequenceMessage, now_ms: u64) -> Option<u64> {
        self.last_peer_activity_ms = now_ms;
        let sent_at = self.in_flight_pings.remove(&pong.seq)?;
        let rtt = now_ms.saturating_sub(sent_at);
        self.last_rtt_ms = Some(rtt);

        if rtt > self.degraded_rtt_threshold_ms {
            if self.state == EngineSessionState::Active {
                if let Ok(next) = self
                    .state
                    .transition(EngineSessionEvent::HeartbeatLostOrHighLatency)
                {
                    self.state = next;
                }
            }
        } else if self.state == EngineSessionState::Degraded {
            if let Ok(next) = self.state.transition(EngineSessionEvent::Recovered) {
                self.state = next;
            }
        }
        Some(rtt)
    }

    /// Evaluate heartbeat loss and idle timeout at `now_ms` (`NET-005`, `FR-014`).
    ///
    /// * If `now_ms - last_peer_activity_ms >= idle_timeout_ms` while `Active` or `Degraded`,
    ///   transitions to `Reconnecting`, closes all active TCP flows, and retains UDP per-flow
    ///   sockets for resumption (`contracts.md` §10).
    /// * If `now_ms - last_peer_activity_ms >= heartbeat_interval_ms` while `Active`, transitions
    ///   to `Degraded`.
    pub fn evaluate_liveness(&mut self, now_ms: u64) -> EngineSessionState {
        let silence_ms = now_ms.saturating_sub(self.last_peer_activity_ms);
        match self.state {
            EngineSessionState::Active | EngineSessionState::Degraded => {
                if silence_ms >= u64::from(self.idle_timeout_ms) {
                    let _ = self.on_connection_lost();
                } else if self.state == EngineSessionState::Active
                    && silence_ms >= u64::from(self.heartbeat_interval_ms).saturating_mul(2)
                {
                    if let Ok(next) = self
                        .state
                        .transition(EngineSessionEvent::HeartbeatLostOrHighLatency)
                    {
                        self.state = next;
                    }
                }
            }
            _ => {}
        }
        self.state
    }

    /// Handle an abrupt QUIC connection drop (`Active | Degraded -> Reconnecting`).
    ///
    /// Closes all active TCP streams while preserving the UDP flow table (`contracts.md` §10,
    /// `FR-014`).
    pub fn on_connection_lost(&mut self) -> Result<EngineSessionState, InvalidSessionTransition> {
        self.state = self.state.transition(EngineSessionEvent::ConnectionLost)?;
        self.tcp.close_all_flows_on_session_drop();
        self.in_flight_pings.clear();
        Ok(self.state)
    }

    /// Record a failed reconnection attempt while in `Reconnecting`. Transitions to `Failed` and
    /// closes UDP sockets once `max_reconnect_attempts` is exhausted (`contracts.md` §12).
    pub fn record_reconnect_failure(&mut self) -> EngineSessionState {
        if self.state == EngineSessionState::Reconnecting {
            self.reconnect_attempts = self.reconnect_attempts.saturating_add(1);
            if self.reconnect_attempts >= self.max_reconnect_attempts {
                if let Ok(next) = self.state.transition(EngineSessionEvent::RetriesExhausted) {
                    self.state = next;
                    self.udp.close_all_flows();
                }
            }
        }
        self.state
    }

    /// User-requested stop (`any non-Failed -> Stopped`, `NET-008`).
    ///
    /// Closes all TCP and UDP flows immediately.
    pub fn stop(&mut self) -> Result<EngineSessionState, InvalidSessionTransition> {
        self.state = self.state.transition(EngineSessionEvent::UserStop)?;
        self.tcp.close_all_flows_on_session_drop();
        self.udp.close_all_flows();
        self.in_flight_pings.clear();
        Ok(self.state)
    }
}
