//! Relay service skeleton: authenticated QUIC session acceptance, `hello` handshake,
//! allocation token validation (`SEC-004`), bounded `jti` replay cache, and `reject` code
//! dispatch (`T-060`, `contracts.md` §§2, 7, 10, 12).
//!
//! ## Architectural Boundary (`SEC-005`, `ADR-003`, `AGENTS.md` §3)
//!
//! **The relay never opens connections to destinations chosen by clients.**
//!
//! * All tunnel sessions arrive inbound from the host engine over TLS-authenticated QUIC (`SEC-010`).
//! * Guest connections arrive inbound on public ports reserved by the control plane (`contracts.md` §7).
//! * Any control frame containing a local destination field (`local_host`, `local_port`, etc.) is
//!   rejected by `gtt_protocol::decode_control_frame` (`INV-1`).
//! * No outbound TCP or UDP dial path exists in this crate.

pub mod admissions;
pub mod limits;
pub mod observability;
pub mod session;
pub mod shutdown;
pub mod tcp;
pub mod udp;

use std::collections::HashMap;
use std::fmt;

use gtt_protocol::{
    decode_control_frame, encode_control_frame, is_valid_uuid, verify_allocation_token,
    ControlMessage, HelloAckMapping, HelloAckMessage, HelloMessage, RejectCode, RejectMessage,
    RelayKeySet, SessionLimits, TokenRefreshAckMessage, TokenRefreshMessage,
    CURRENT_PROTOCOL_VERSION, DEFAULT_TOKEN_ISSUER,
};

pub use admissions::{
    canonicalize_guest_ip, RelayAdmissionEnforcer, RelayAdmissionEntry, RelayAdmissionMetrics,
    RelayAdmissionSyncDelta, RevocationLatencySample, RevocationTimingStats,
    DEFAULT_MAX_RELAY_ADMISSIONS, MAX_RELAY_ADMISSIONS_CEILING, MAX_REVOCATION_SAMPLES,
};
pub use observability::{format_relay_prometheus, RelayTelemetrySnapshot};

pub use limits::{
    BackpressureDropReason, BoundedSessionQueue, GlobalRelayMemoryBudget, RelayCapacityConfig,
    RelayCapacityError, RelayLimitMetrics, TokenBucketLimiter, DEFAULT_GLOBAL_MAX_QUEUED_BYTES,
    DEFAULT_MAX_QUEUE_BYTES, DEFAULT_MAX_QUEUE_FRAMES, DEFAULT_MAX_TOTAL_FLOWS,
    DEFAULT_SESSION_BANDWIDTH_BYTES_PER_SEC,
};
pub use session::{RelaySessionConnectivity, RelayTunnelSession, DEFAULT_GRACE_PERIOD_MS};
pub use shutdown::{RelayAllocationSupervisor, RelayBoundAllocation, ShutdownReport};
pub use tcp::{
    bridge_tcp_bidirectional, relay_forward_tcp_connection, RelayTcpError, RelayTcpMetrics,
    RelayTcpSession, DEFAULT_TCP_IO_TIMEOUT, DEFAULT_TCP_OPEN_TIMEOUT, TCP_COPY_CHUNK_BYTES,
};
pub use udp::{RelayUdpMetrics, RelayUdpSession, DEFAULT_UDP_FLOW_TIMEOUT_MS};

/// Required ALPN protocol identifier (`contracts.md` §10).
pub const RELAY_ALPN: &str = "gtt/1";

/// Default heartbeat interval advertised in `hello_ack` (`5_000 ms`, *(proposal)*).
pub const DEFAULT_HEARTBEAT_INTERVAL_MS: u32 = 5_000;

/// Default idle timeout advertised in `hello_ack` (`15_000 ms`, *(proposal)*).
pub const DEFAULT_IDLE_TIMEOUT_MS: u32 = 15_000;

/// Default per-session flow limits (`contracts.md` §10, *(proposal)*).
pub const DEFAULT_MAX_TCP_FLOWS: u16 = 64;

/// Default per-session UDP flow limit (`contracts.md` §10, *(proposal)*).
pub const DEFAULT_MAX_UDP_FLOWS: u16 = 64;

/// Default maximum concurrent allocations on a single relay instance.
pub const DEFAULT_MAX_ALLOCATIONS: usize = 256;

/// Upper ceiling on `max_allocations` configuration.
pub const MAX_ALLOCATIONS_CEILING: usize = 10_000;

/// Default maximum `jti` entries in the in-memory replay cache (`AGENTS.md` §4).
pub const DEFAULT_MAX_REPLAY_ENTRIES: usize = 16_384;

/// Upper ceiling on `max_replay_entries` configuration.
pub const MAX_REPLAY_ENTRIES_CEILING: usize = 100_000;

/// Relay instance configuration (`T-060`, `SEC-010`, `contracts.md` §10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayConfig {
    /// This relay node's UUID (`aud` claim in allocation tokens, `contracts.md` §2).
    pub relay_id: String,
    /// Expected token issuer (`iss`).
    pub expected_issuer: String,
    /// Required QUIC TLS ALPN identifier (`"gtt/1"`).
    pub alpn_protocol: String,
    /// Requires TLS 1.3 authenticated encryption on all QUIC connections (`SEC-010`).
    pub require_tls: bool,
    /// Maximum concurrent active sessions on this relay (`1..=10_000`).
    pub max_allocations: usize,
    /// Maximum entries in the `jti` replay cache (`1..=100_000`, `AGENTS.md` §4).
    pub max_replay_entries: usize,
    /// Heartbeat interval advertised in `hello_ack` (`100..=300_000 ms`).
    pub heartbeat_interval_ms: u32,
    /// Idle timeout advertised in `hello_ack` (`> heartbeat_interval_ms`, `<= 600_000 ms`).
    pub idle_timeout_ms: u32,
    /// Per-session flow caps advertised in `hello_ack`.
    pub session_limits: SessionLimits,
}

impl RelayConfig {
    /// Construct a [`RelayConfig`] with standard `contracts.md` §10 proposal defaults.
    pub fn new(relay_id: impl Into<String>) -> Self {
        Self {
            relay_id: relay_id.into(),
            expected_issuer: DEFAULT_TOKEN_ISSUER.to_owned(),
            alpn_protocol: RELAY_ALPN.to_owned(),
            require_tls: true,
            max_allocations: DEFAULT_MAX_ALLOCATIONS,
            max_replay_entries: DEFAULT_MAX_REPLAY_ENTRIES,
            heartbeat_interval_ms: DEFAULT_HEARTBEAT_INTERVAL_MS,
            idle_timeout_ms: DEFAULT_IDLE_TIMEOUT_MS,
            session_limits: SessionLimits {
                max_tcp_flows: DEFAULT_MAX_TCP_FLOWS,
                max_udp_flows: DEFAULT_MAX_UDP_FLOWS,
            },
        }
    }

    /// Validate all configuration invariants (`SEC-010`, `AGENTS.md` §4).
    pub fn validate(&self) -> Result<(), RelayConfigError> {
        if !is_valid_uuid(&self.relay_id) {
            return Err(RelayConfigError(
                "relay_id must be a valid UUID (contracts.md §2)",
            ));
        }
        if self.expected_issuer.trim().is_empty() || self.expected_issuer.len() > 64 {
            return Err(RelayConfigError(
                "expected_issuer must be 1 to 64 characters",
            ));
        }
        if !self.require_tls {
            return Err(RelayConfigError(
                "unauthenticated QUIC without TLS is forbidden (SEC-010)",
            ));
        }
        if self.alpn_protocol != RELAY_ALPN {
            return Err(RelayConfigError("alpn_protocol must be \"gtt/1\""));
        }
        if !(1..=MAX_ALLOCATIONS_CEILING).contains(&self.max_allocations) {
            return Err(RelayConfigError(
                "max_allocations must be between 1 and 10000",
            ));
        }
        if !(1..=MAX_REPLAY_ENTRIES_CEILING).contains(&self.max_replay_entries) {
            return Err(RelayConfigError(
                "max_replay_entries must be between 1 and 100000",
            ));
        }
        if !(100..=300_000).contains(&self.heartbeat_interval_ms) {
            return Err(RelayConfigError(
                "heartbeat_interval_ms must be between 100 and 300000",
            ));
        }
        if self.idle_timeout_ms <= self.heartbeat_interval_ms || self.idle_timeout_ms > 600_000 {
            return Err(RelayConfigError(
                "idle_timeout_ms must be > heartbeat_interval_ms and <= 600000",
            ));
        }
        if !(1..=4096).contains(&self.session_limits.max_tcp_flows)
            || !(1..=4096).contains(&self.session_limits.max_udp_flows)
        {
            return Err(RelayConfigError(
                "session_limits max_tcp_flows and max_udp_flows must be between 1 and 4096",
            ));
        }
        Ok(())
    }
}

/// Invalid [`RelayConfig`] error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayConfigError(pub &'static str);

impl fmt::Display for RelayConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid relay configuration: {}", self.0)
    }
}

impl std::error::Error for RelayConfigError {}

/// Bounded in-memory `jti` replay cache (`contracts.md` §2, `SEC-004`, `AGENTS.md` §4).
///
/// Tracks seen token identifiers (`jti`) until their `exp` timestamp so a captured allocation
/// token cannot be replayed to open a second session.
#[derive(Debug, Clone)]
pub struct ReplayCache {
    max_entries: usize,
    entries: HashMap<String, i64>,
}

/// Reason a `jti` insertion into [`ReplayCache`] was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayError {
    /// `jti` has already been consumed and has not yet expired (`SEC-004`).
    DuplicateJti,
    /// Cache reached `max_entries` even after evicting expired entries (`AGENTS.md` §4).
    CacheFull,
}

impl ReplayCache {
    /// Construct a bounded [`ReplayCache`].
    pub fn new(max_entries: usize) -> Self {
        Self {
            max_entries: max_entries.max(1),
            entries: HashMap::new(),
        }
    }

    /// Number of non-evicted `jti` entries currently stored.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if no `jti` entries are stored.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Evict all `jti` entries whose `exp_epoch_s <= now_epoch_s`.
    pub fn prune_expired(&mut self, now_epoch_s: i64) -> usize {
        let before = self.entries.len();
        self.entries.retain(|_, &mut exp| exp > now_epoch_s);
        before.saturating_sub(self.entries.len())
    }

    /// Check that `jti` has not been used, evicting expired entries first, and record `(jti, exp)`
    /// on success.
    pub fn check_and_insert(
        &mut self,
        jti: &str,
        exp_epoch_s: i64,
        now_epoch_s: i64,
    ) -> Result<(), ReplayError> {
        self.prune_expired(now_epoch_s);

        if let Some(&existing_exp) = self.entries.get(jti) {
            if existing_exp > now_epoch_s {
                return Err(ReplayError::DuplicateJti);
            }
        }
        if self.entries.len() >= self.max_entries {
            return Err(ReplayError::CacheFull);
        }
        self.entries.insert(jti.to_owned(), exp_epoch_s);
        Ok(())
    }
}

/// Lifecycle state of an allocation reservation on the relay (`contracts.md` §12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReservationState {
    /// Reserved by the control plane; awaiting first successful `hello`.
    Reserved,
    /// Active tunnel session authenticated via `hello`.
    Active {
        /// Session UUID returned in `hello_ack`.
        session_id: String,
    },
    /// Released by stop or failover.
    Released,
    /// Expired before `hello` or past allocation lifetime.
    Expired,
}

/// Relay-side record of a control-plane port reservation (`contracts.md` §§7, 10, 12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllocationReservation {
    /// Allocation UUID (`aid`).
    pub allocation_id: String,
    /// Tunnel UUID (`sub`).
    pub tunnel_id: String,
    /// Reservation expiration timestamp (Unix epoch seconds).
    pub expires_at_epoch_s: i64,
    /// Reserved public port mappings in wire order.
    pub mappings: Vec<HelloAckMapping>,
    /// Current reservation state.
    pub state: ReservationState,
}

/// Metadata describing an inbound QUIC connection accepted by the relay listener (`SEC-010`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuicConnectionMeta {
    /// Whether the QUIC connection completed TLS 1.3 handshake with valid cipher suite (`SEC-010`).
    pub tls_authenticated: bool,
    /// ALPN protocol negotiated during the QUIC TLS handshake.
    pub negotiated_alpn: String,
    /// Session UUID assigned by the relay for this connection (`hello_ack.session_id`).
    pub assigned_session_id: String,
}

/// Outcome of processing the first control-stream frame on an inbound QUIC connection (`T-060`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandshakeOutcome {
    /// `hello` succeeded; relay sends `encoded_frame` (`hello_ack`) and transitions the
    /// reservation to [`ReservationState::Active`].
    Accepted {
        /// Parsed `hello_ack` message.
        ack: HelloAckMessage,
        /// Length-prefixed wire bytes to write to the control stream.
        encoded_frame: Vec<u8>,
        /// Authenticated allocation UUID.
        allocation_id: String,
        /// Authenticated tunnel UUID.
        tunnel_id: String,
    },
    /// `hello` was rejected; relay writes `encoded_frame` (`reject`) and closes the QUIC
    /// connection (`contracts.md` §10).
    Rejected {
        /// Structured `reject` message with the exact `RejectCode`.
        reject: RejectMessage,
        /// Length-prefixed wire bytes to write before closing the connection.
        encoded_frame: Vec<u8>,
    },
    /// Connection was aborted at the transport or framing layer before a `reject` message could
    /// be sent (e.g. unauthenticated QUIC/TLS `SEC-010`, wrong ALPN, malformed length prefix, or
    /// first message was not `hello`).
    Aborted {
        /// Diagnostic reason (no secrets or payload bytes, `PRIV-002`).
        reason: String,
    },
}

/// Relay node state machine managing verification keys, `jti` replay cache, reservations, and
/// inbound QUIC `hello` handshakes (`T-060`, `SEC-004`, `SEC-005`, `SEC-010`).
#[derive(Debug)]
pub struct RelayNode {
    config: RelayConfig,
    key_set: RelayKeySet,
    replay_cache: ReplayCache,
    reservations: HashMap<String, AllocationReservation>,
    draining: bool,
    drain_retry_after_ms: Option<u64>,
}

impl RelayNode {
    /// Construct a validated [`RelayNode`].
    pub fn new(config: RelayConfig, key_set: RelayKeySet) -> Result<Self, RelayConfigError> {
        config.validate()?;
        let replay_cache = ReplayCache::new(config.max_replay_entries);
        Ok(Self {
            config,
            key_set,
            replay_cache,
            reservations: HashMap::new(),
            draining: false,
            drain_retry_after_ms: None,
        })
    }

    /// Borrow the relay configuration.
    pub fn config(&self) -> &RelayConfig {
        &self.config
    }

    /// Update the verification key set pulled from `/internal/v1/relays/{relay_id}/state`
    /// (`contracts.md` §7, `T-050`).
    pub fn update_verification_keys(&mut self, key_set: RelayKeySet) {
        self.key_set = key_set;
    }

    /// Set or clear the relay drain flag (`contracts.md` §§7, 10).
    pub fn set_draining(&mut self, draining: bool, retry_after_ms: Option<u64>) {
        self.draining = draining;
        self.drain_retry_after_ms = retry_after_ms;
    }

    /// True if the relay is currently draining.
    pub fn is_draining(&self) -> bool {
        self.draining
    }

    /// Upsert an allocation reservation from the control-plane state poll (`contracts.md` §7).
    pub fn upsert_reservation(
        &mut self,
        reservation: AllocationReservation,
    ) -> Result<(), &'static str> {
        if !is_valid_uuid(&reservation.allocation_id) || !is_valid_uuid(&reservation.tunnel_id) {
            return Err("allocation_id and tunnel_id must be valid UUIDs");
        }
        if reservation.mappings.is_empty() {
            return Err("reservation must include at least one mapping");
        }
        self.reservations
            .insert(reservation.allocation_id.clone(), reservation);
        Ok(())
    }

    /// Release an allocation reservation (`contracts.md` §§7, 12).
    pub fn release_reservation(&mut self, allocation_id: &str) -> bool {
        if let Some(res) = self.reservations.get_mut(allocation_id) {
            res.state = ReservationState::Released;
            true
        } else {
            false
        }
    }

    /// Look up a reservation by `allocation_id`.
    pub fn reservation(&self, allocation_id: &str) -> Option<&AllocationReservation> {
        self.reservations.get(allocation_id)
    }

    /// Count currently active sessions on this relay.
    pub fn active_session_count(&self) -> usize {
        self.reservations
            .values()
            .filter(|r| matches!(r.state, ReservationState::Active { .. }))
            .count()
    }

    /// Borrow the `jti` replay cache.
    pub fn replay_cache(&self) -> &ReplayCache {
        &self.replay_cache
    }

    /// Accept an inbound QUIC connection and process its initial length-prefixed control frame
    /// (`contracts.md` §10, `SEC-004`, `SEC-005`, `SEC-010`).
    pub fn accept_hello_frame(
        &mut self,
        conn_meta: &QuicConnectionMeta,
        raw_control_frame: &[u8],
        now_epoch_s: i64,
    ) -> HandshakeOutcome {
        // SEC-010: refuse unauthenticated QUIC or wrong ALPN before parsing any payload.
        if !conn_meta.tls_authenticated {
            return HandshakeOutcome::Aborted {
                reason: "unauthenticated QUIC connection refused (SEC-010)".to_owned(),
            };
        }
        if conn_meta.negotiated_alpn != self.config.alpn_protocol {
            return HandshakeOutcome::Aborted {
                reason: format!(
                    "unsupported ALPN {:?}, expected {:?}",
                    conn_meta.negotiated_alpn, self.config.alpn_protocol
                ),
            };
        }
        if !is_valid_uuid(&conn_meta.assigned_session_id) {
            return HandshakeOutcome::Aborted {
                reason: "assigned_session_id must be a valid UUID".to_owned(),
            };
        }

        let (control_msg, consumed) = match decode_control_frame(raw_control_frame) {
            Ok(res) => res,
            Err(err) => {
                return HandshakeOutcome::Aborted {
                    reason: format!("invalid control frame: {err}"),
                };
            }
        };
        if consumed != raw_control_frame.len() {
            return HandshakeOutcome::Aborted {
                reason: "unexpected trailing bytes after initial hello frame".to_owned(),
            };
        }

        let hello = match control_msg {
            ControlMessage::Hello(h) => h,
            _ => {
                return HandshakeOutcome::Aborted {
                    reason: "first control message on stream must be 'hello'".to_owned(),
                };
            }
        };

        self.evaluate_hello(&conn_meta.assigned_session_id, &hello, now_epoch_s)
    }

    /// Evaluate a parsed [`HelloMessage`] and produce [`HandshakeOutcome`].
    pub fn evaluate_hello(
        &mut self,
        assigned_session_id: &str,
        hello: &HelloMessage,
        now_epoch_s: i64,
    ) -> HandshakeOutcome {
        // 1. Protocol version check (`contracts.md` §§10, 14).
        if hello.protocol_version != CURRENT_PROTOCOL_VERSION {
            return build_reject(RejectCode::VersionUnsupported, None);
        }

        // 2. Client version string sanity check (`contracts.md` §10).
        if hello.client_version.trim().is_empty()
            || hello.client_version.len() > 64
            || hello.client_version.chars().any(char::is_control)
        {
            return build_reject(RejectCode::TokenInvalid, None);
        }

        // 3. Relay draining check (`contracts.md` §10).
        if self.draining {
            return build_reject(RejectCode::RelayDraining, self.drain_retry_after_ms);
        }

        // 4. Relay capacity check (`contracts.md` §10).
        if self.active_session_count() >= self.config.max_allocations {
            return build_reject(RejectCode::Capacity, None);
        }

        // 5. Cryptographic signature, kid, iss, aud, and exp verification (`SEC-004`).
        let (_header, claims) = match verify_allocation_token(
            &hello.token,
            &self.key_set,
            &self.config.expected_issuer,
            &self.config.relay_id,
            now_epoch_s,
        ) {
            Ok(verified) => verified,
            Err(err) => {
                return build_reject(err.to_reject_code(), None);
            }
        };

        // 6. Allocation reservation lookup & state check (`contracts.md` §§10, 12).
        let reservation_valid = match self.reservations.get_mut(&claims.aid) {
            None => false,
            Some(res) => {
                if now_epoch_s >= res.expires_at_epoch_s {
                    res.state = ReservationState::Expired;
                    false
                } else {
                    match res.state {
                        ReservationState::Released | ReservationState::Expired => false,
                        ReservationState::Reserved | ReservationState::Active { .. } => {
                            res.tunnel_id == claims.sub && res.mappings == claims.mappings
                        }
                    }
                }
            }
        };

        if !reservation_valid {
            return build_reject(RejectCode::AllocationNotFound, None);
        }

        // 7. Replay cache check (`jti` uniqueness until `exp`, `SEC-004`).
        match self
            .replay_cache
            .check_and_insert(&claims.jti, claims.exp, now_epoch_s)
        {
            Ok(()) => {}
            Err(ReplayError::DuplicateJti) => {
                return build_reject(RejectCode::TokenInvalid, None);
            }
            Err(ReplayError::CacheFull) => {
                return build_reject(RejectCode::Capacity, None);
            }
        }

        // 8. Transition reservation to Active and emit `hello_ack`.
        if let Some(res) = self.reservations.get_mut(&claims.aid) {
            res.state = ReservationState::Active {
                session_id: assigned_session_id.to_owned(),
            };
        }

        let ack = HelloAckMessage {
            session_id: assigned_session_id.to_owned(),
            heartbeat_interval_ms: self.config.heartbeat_interval_ms,
            idle_timeout_ms: self.config.idle_timeout_ms,
            limits: self.config.session_limits,
            mappings: claims.mappings,
        };

        match encode_control_frame(&ControlMessage::HelloAck(ack.clone())) {
            Ok(encoded_frame) => HandshakeOutcome::Accepted {
                ack,
                encoded_frame,
                allocation_id: claims.aid,
                tunnel_id: claims.sub,
            },
            Err(err) => HandshakeOutcome::Aborted {
                reason: format!("failed to encode hello_ack: {err}"),
            },
        }
    }

    /// Validate an in-session `token_refresh` control message (`contracts.md` §10).
    pub fn handle_token_refresh(
        &mut self,
        allocation_id: &str,
        msg: &TokenRefreshMessage,
        expires_at_rfc3339: String,
        now_epoch_s: i64,
    ) -> Result<TokenRefreshAckMessage, RejectCode> {
        let (_header, claims) = verify_allocation_token(
            &msg.token,
            &self.key_set,
            &self.config.expected_issuer,
            &self.config.relay_id,
            now_epoch_s,
        )
        .map_err(|e| e.to_reject_code())?;

        if claims.aid != allocation_id {
            return Err(RejectCode::TokenInvalid);
        }

        let Some(res) = self.reservations.get_mut(allocation_id) else {
            return Err(RejectCode::AllocationNotFound);
        };
        if !matches!(res.state, ReservationState::Active { .. }) || res.tunnel_id != claims.sub {
            return Err(RejectCode::AllocationNotFound);
        }

        match self
            .replay_cache
            .check_and_insert(&claims.jti, claims.exp, now_epoch_s)
        {
            Ok(()) => {}
            Err(ReplayError::DuplicateJti) => return Err(RejectCode::TokenInvalid),
            Err(ReplayError::CacheFull) => return Err(RejectCode::Capacity),
        }

        res.expires_at_epoch_s = claims.exp;
        Ok(TokenRefreshAckMessage {
            expires_at: expires_at_rfc3339,
        })
    }
}

fn build_reject(code: RejectCode, retry_after_ms: Option<u64>) -> HandshakeOutcome {
    let reject = RejectMessage {
        code,
        retry_after_ms,
    };
    match encode_control_frame(&ControlMessage::Reject(reject)) {
        Ok(encoded_frame) => HandshakeOutcome::Rejected {
            reject,
            encoded_frame,
        },
        Err(err) => HandshakeOutcome::Aborted {
            reason: format!("failed to encode reject message: {err}"),
        },
    }
}
