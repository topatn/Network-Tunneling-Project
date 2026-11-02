//! Engine session state machine (`contracts.md` §12).
//!
//! Local server reachability and players seen are **separate** indicators (`DESIGN.md` §6,
//! `user-flows.md`), never folded into [`EngineSessionState`].

use std::fmt;

use crate::control::RejectCode;

/// Engine tunnel session states (`contracts.md` §12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineSessionState {
    /// Not started.
    Idle,
    /// Establishing QUIC transport to the relay.
    Connecting,
    /// QUIC established; awaiting `hello_ack` or `reject` on the control stream.
    Authenticating,
    /// Session authenticated and healthy.
    Active,
    /// Heartbeat loss or latency threshold exceeded while connected.
    Degraded,
    /// Recovering from a dropped connection or retryable rejection within the grace window.
    Reconnecting,
    /// Terminal failure (`token_invalid`, `version_unsupported`, or retries exhausted).
    Failed,
    /// Terminal user-requested stop.
    Stopped,
}

impl EngineSessionState {
    /// True when the session has reached a terminal state (`Failed` or `Stopped`).
    ///
    /// `contracts.md` §12: `Failed` and `Stopped` are terminal for the session; a new start is
    /// a new session.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Failed | Self::Stopped)
    }

    /// Apply an [`EngineSessionEvent`] according to the transition table in `contracts.md` §12.
    pub fn transition(self, event: EngineSessionEvent) -> Result<Self, InvalidSessionTransition> {
        match (self, event) {
            // `any | user stop | Stopped` — except that once `Failed`, the session is already
            // terminal (`Failed and Stopped are terminal for the session`). Stopping an already
            // `Stopped` session is idempotent.
            (Self::Stopped, EngineSessionEvent::UserStop) => Ok(Self::Stopped),
            (state, EngineSessionEvent::UserStop) if !state.is_terminal() => Ok(Self::Stopped),

            (Self::Idle, EngineSessionEvent::StartWithToken) => Ok(Self::Connecting),
            (Self::Connecting, EngineSessionEvent::QuicEstablished) => Ok(Self::Authenticating),
            (Self::Authenticating, EngineSessionEvent::HelloAckReceived) => Ok(Self::Active),
            (Self::Authenticating, EngineSessionEvent::Rejected(code)) => {
                if code.is_retryable() {
                    Ok(Self::Reconnecting)
                } else {
                    Ok(Self::Failed)
                }
            }
            (Self::Active, EngineSessionEvent::HeartbeatLostOrHighLatency) => Ok(Self::Degraded),
            (Self::Degraded, EngineSessionEvent::Recovered) => Ok(Self::Active),
            (Self::Active | Self::Degraded, EngineSessionEvent::ConnectionLost) => {
                Ok(Self::Reconnecting)
            }
            (Self::Reconnecting, EngineSessionEvent::ReconnectSucceeded) => Ok(Self::Active),
            (Self::Reconnecting, EngineSessionEvent::RetriesExhausted) => Ok(Self::Failed),

            (from, event) => Err(InvalidSessionTransition { from, event }),
        }
    }
}

/// Events driving [`EngineSessionState`] transitions (`contracts.md` §12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineSessionEvent {
    /// Host initiates session with an allocation token.
    StartWithToken,
    /// QUIC connection to the relay completed TLS handshake.
    QuicEstablished,
    /// Relay replied with `hello_ack`.
    HelloAckReceived,
    /// Relay replied with `reject`.
    Rejected(RejectCode),
    /// Heartbeat missed or RTT exceeded threshold.
    HeartbeatLostOrHighLatency,
    /// Heartbeat and RTT recovered.
    Recovered,
    /// QUIC transport lost during `Active` or `Degraded`.
    ConnectionLost,
    /// Reconnection handshake succeeded (`hello_ack`).
    ReconnectSucceeded,
    /// Reconnection attempts or grace window exhausted.
    RetriesExhausted,
    /// Host stopped the tunnel.
    UserStop,
}

/// Returned when an [`EngineSessionEvent`] is not valid from the current [`EngineSessionState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidSessionTransition {
    /// Current state.
    pub from: EngineSessionState,
    /// Rejected event.
    pub event: EngineSessionEvent,
}

impl fmt::Display for InvalidSessionTransition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid engine session transition from {:?} on {:?}",
            self.from, self.event
        )
    }
}

impl std::error::Error for InvalidSessionTransition {}
