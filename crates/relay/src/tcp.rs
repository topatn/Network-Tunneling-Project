//! Relay-side TCP forwarding path (`T-061`, `contracts.md` §11, `NET-001`, `NET-003`, `SEC-005`).
//!
//! ## Wire protocol (`contracts.md` §11)
//!
//! 1. A guest connects inbound to a reserved public TCP listener port on the relay associated with
//!    `mapping_index` (`HelloAckMapping` with `Protocol::Tcp`).
//! 2. The relay checks that the session is active, that `mapping_index` is an authorized TCP
//!    mapping, and that `active_tcp_flows < max_tcp_flows`.
//! 3. On the newly opened bidirectional stream to the host engine, the relay writes the 3-byte
//!    [`TcpOpenHeader`] (`[0x01, mapping_index_hi, mapping_index_lo]`) and reads the 1-byte
//!    [`TcpOpenStatus`] reply from the engine (`0x00` connected, `0x01` refused, `0x02` limit
//!    reached).
//! 4. On `0x00` (`Connected`), both directions forward raw bytes with bounded chunk buffers and
//!    propagate half-close (`EOF` -> `Shutdown::Write`) independently in each direction (`NET-001`).
//! 5. On `0x01` (`Refused`), `0x02` (`LimitReached`), or any protocol/I/O error, the relay closes
//!    the guest TCP connection immediately.
//!
//! ## Architectural boundary (`SEC-005`)
//!
//! This module never opens an outbound connection to any client-supplied destination. Guest
//! connections arrive inbound on reserved public listeners, and tunnel streams are multiplexed over
//! the engine's outbound-initiated session.

use std::collections::HashSet;
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Shutdown, TcpStream};
use std::thread;
use std::time::Duration;

use gtt_protocol::{
    FrameError, HelloAckMapping, Protocol, TcpOpenHeader, TcpOpenStatus, TCP_OPEN_HEADER_LEN,
};

/// Bounded buffer size used per directional TCP copy loop (`16 KiB`, `AGENTS.md` §4).
pub const TCP_COPY_CHUNK_BYTES: usize = 16_384;

/// Default timeout waiting for the engine's 1-byte [`TcpOpenStatus`] reply (`5 seconds`).
pub const DEFAULT_TCP_OPEN_TIMEOUT: Duration = Duration::from_secs(5);

/// Default read/write timeout on forwarded TCP streams (`30 seconds`, `AGENTS.md` §4).
pub const DEFAULT_TCP_IO_TIMEOUT: Duration = Duration::from_secs(30);

/// Metrics tracked by the relay TCP forwarding path (`contracts.md` §§10–11, `NET-001`, `NET-006`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RelayTcpMetrics {
    /// Currently open TCP flows for this session.
    pub active_tcp_flows: u16,
    /// Total guest TCP connections accepted and opened toward the engine (`0x00`).
    pub total_connected: u64,
    /// Total guest TCP connections refused by the engine (`0x01`) or due to invalid mapping.
    pub total_refused: u64,
    /// Total guest TCP connections refused due to relay or engine `max_tcp_flows` cap (`0x02`).
    pub total_limit_reached: u64,
    /// Total bytes forwarded from guest to engine across completed streams.
    pub bytes_guest_to_engine: u64,
    /// Total bytes forwarded from engine to guest across completed streams.
    pub bytes_engine_to_guest: u64,
}

/// Errors returned by relay-side TCP stream opening or forwarding.
#[derive(Debug)]
pub enum RelayTcpError {
    /// Requested `mapping_index` does not exist in `hello_ack.mappings` or is not `Protocol::Tcp`.
    InvalidTcpMapping {
        /// Mapping index requested.
        mapping_index: u16,
    },
    /// Session has reached `max_tcp_flows`.
    FlowLimitReached {
        /// Configured `max_tcp_flows` limit.
        max_tcp_flows: u16,
    },
    /// Engine replied with `0x01` (`Refused`) or `0x02` (`LimitReached`).
    EngineRejected(TcpOpenStatus),
    /// Malformed status byte from the engine (`NET-007`).
    Protocol(FrameError),
    /// Underlying socket/stream I/O error or timeout.
    Io(io::Error),
}

impl fmt::Display for RelayTcpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTcpMapping { mapping_index } => {
                write!(
                    f,
                    "mapping_index {mapping_index} is not an active TCP mapping"
                )
            }
            Self::FlowLimitReached { max_tcp_flows } => {
                write!(f, "relay session TCP flow limit ({max_tcp_flows}) reached")
            }
            Self::EngineRejected(status) => {
                write!(f, "engine rejected TCP stream open with status {status:?}")
            }
            Self::Protocol(err) => write!(f, "protocol error on TCP open: {err}"),
            Self::Io(err) => write!(f, "TCP forwarding I/O error: {err}"),
        }
    }
}

impl std::error::Error for RelayTcpError {}

/// Per-session relay TCP flow manager enforcing `max_tcp_flows` and tracking distinct guest sources
/// for `flow_summary` (`contracts.md` §§10–11).
#[derive(Debug, Clone)]
pub struct RelayTcpSession {
    tcp_mapping_indexes: HashSet<u16>,
    max_tcp_flows: u16,
    distinct_sources: HashSet<IpAddr>,
    max_distinct_sources: usize,
    metrics: RelayTcpMetrics,
}

impl RelayTcpSession {
    /// Construct a [`RelayTcpSession`] from the session's `HelloAckMapping` list and `max_tcp_flows`.
    pub fn new(mappings: &[HelloAckMapping], max_tcp_flows: u16) -> Self {
        let tcp_mapping_indexes = mappings
            .iter()
            .filter(|m| m.protocol == Protocol::Tcp)
            .map(|m| m.index)
            .collect();
        Self {
            tcp_mapping_indexes,
            max_tcp_flows: max_tcp_flows.clamp(1, 4096),
            distinct_sources: HashSet::new(),
            max_distinct_sources: 4096,
            metrics: RelayTcpMetrics::default(),
        }
    }

    /// Borrow current TCP forwarding metrics.
    pub fn metrics(&self) -> &RelayTcpMetrics {
        &self.metrics
    }

    /// Count of distinct guest source IP addresses observed (`contracts.md` §10 `flow_summary`).
    ///
    /// Note (`PRIV-002`): raw guest IP addresses are never logged or exported; only the cardinality
    /// is exposed for `flow_summary.distinct_sources`.
    pub fn distinct_source_count(&self) -> u32 {
        u32::try_from(self.distinct_sources.len()).unwrap_or(u32::MAX)
    }

    /// Reserve a TCP flow slot and encode the 3-byte [`TcpOpenHeader`] to send on a new stream to
    /// the engine (`contracts.md` §11).
    pub fn begin_tcp_open(
        &mut self,
        mapping_index: u16,
        guest_ip: IpAddr,
    ) -> Result<[u8; TCP_OPEN_HEADER_LEN], RelayTcpError> {
        if !self.tcp_mapping_indexes.contains(&mapping_index) {
            self.metrics.total_refused = self.metrics.total_refused.saturating_add(1);
            return Err(RelayTcpError::InvalidTcpMapping { mapping_index });
        }
        if self.metrics.active_tcp_flows >= self.max_tcp_flows {
            self.metrics.total_limit_reached = self.metrics.total_limit_reached.saturating_add(1);
            return Err(RelayTcpError::FlowLimitReached {
                max_tcp_flows: self.max_tcp_flows,
            });
        }

        self.metrics.active_tcp_flows = self.metrics.active_tcp_flows.saturating_add(1);
        if self.distinct_sources.len() < self.max_distinct_sources {
            self.distinct_sources.insert(guest_ip);
        }

        Ok(TcpOpenHeader { mapping_index }.encode())
    }

    /// Process the 1-byte [`TcpOpenStatus`] reply from the engine after [`Self::begin_tcp_open`].
    ///
    /// If the engine replied with anything other than [`TcpOpenStatus::Connected`] (`0x00`), the
    /// reserved flow slot is immediately released and an error is returned so the caller closes
    /// the guest socket (`contracts.md` §11).
    pub fn complete_tcp_open(&mut self, status_byte: u8) -> Result<TcpOpenStatus, RelayTcpError> {
        match TcpOpenStatus::from_byte(status_byte) {
            Ok(TcpOpenStatus::Connected) => {
                self.metrics.total_connected = self.metrics.total_connected.saturating_add(1);
                Ok(TcpOpenStatus::Connected)
            }
            Ok(TcpOpenStatus::Refused) => {
                self.metrics.active_tcp_flows = self.metrics.active_tcp_flows.saturating_sub(1);
                self.metrics.total_refused = self.metrics.total_refused.saturating_add(1);
                Err(RelayTcpError::EngineRejected(TcpOpenStatus::Refused))
            }
            Ok(TcpOpenStatus::LimitReached) => {
                self.metrics.active_tcp_flows = self.metrics.active_tcp_flows.saturating_sub(1);
                self.metrics.total_limit_reached =
                    self.metrics.total_limit_reached.saturating_add(1);
                Err(RelayTcpError::EngineRejected(TcpOpenStatus::LimitReached))
            }
            Err(err) => {
                self.metrics.active_tcp_flows = self.metrics.active_tcp_flows.saturating_sub(1);
                self.metrics.total_refused = self.metrics.total_refused.saturating_add(1);
                Err(RelayTcpError::Protocol(err))
            }
        }
    }

    /// Record completion of an active TCP flow and add its byte counters.
    pub fn finish_tcp_flow(&mut self, bytes_guest_to_engine: u64, bytes_engine_to_guest: u64) {
        self.metrics.active_tcp_flows = self.metrics.active_tcp_flows.saturating_sub(1);
        self.metrics.bytes_guest_to_engine = self
            .metrics
            .bytes_guest_to_engine
            .saturating_add(bytes_guest_to_engine);
        self.metrics.bytes_engine_to_guest = self
            .metrics
            .bytes_engine_to_guest
            .saturating_add(bytes_engine_to_guest);
    }

    /// Abort a flow that failed after [`Self::begin_tcp_open`] before [`Self::complete_tcp_open`].
    pub fn abort_pending_open(&mut self) {
        self.metrics.active_tcp_flows = self.metrics.active_tcp_flows.saturating_sub(1);
        self.metrics.total_refused = self.metrics.total_refused.saturating_add(1);
    }

    /// Reset all active TCP flows when the tunnel session drops (`contracts.md` §10: established
    /// TCP connections are lost when the tunnel connection drops).
    pub fn close_all_flows_on_session_drop(&mut self) -> u16 {
        let closed = self.metrics.active_tcp_flows;
        self.metrics.active_tcp_flows = 0;
        closed
    }
}

/// Perform the relay-side `tcp_open` handshake over `tunnel_stream` and, if accepted (`0x00`),
/// bridge `guest_stream` and `tunnel_stream` bidirectionally with half-close propagation (`NET-001`).
///
/// Returns `(bytes_guest_to_engine, bytes_engine_to_guest)` on a completed `0x00` session.
pub fn relay_forward_tcp_connection(
    session: &mut RelayTcpSession,
    mapping_index: u16,
    guest_stream: TcpStream,
    mut tunnel_stream: TcpStream,
    io_timeout: Duration,
) -> Result<(u64, u64), RelayTcpError> {
    let guest_ip = guest_stream
        .peer_addr()
        .map(|a| a.ip())
        .map_err(RelayTcpError::Io)?;

    let header = session.begin_tcp_open(mapping_index, guest_ip)?;

    if let Err(err) = configure_stream_timeouts(&guest_stream, &tunnel_stream, io_timeout) {
        session.abort_pending_open();
        let _ = guest_stream.shutdown(Shutdown::Both);
        let _ = tunnel_stream.shutdown(Shutdown::Both);
        return Err(RelayTcpError::Io(err));
    }

    if let Err(err) = tunnel_stream
        .write_all(&header)
        .and_then(|()| tunnel_stream.flush())
    {
        session.abort_pending_open();
        let _ = guest_stream.shutdown(Shutdown::Both);
        let _ = tunnel_stream.shutdown(Shutdown::Both);
        return Err(RelayTcpError::Io(err));
    }

    let mut status_buf = [0u8; 1];
    if let Err(err) = tunnel_stream.read_exact(&mut status_buf) {
        session.abort_pending_open();
        let _ = guest_stream.shutdown(Shutdown::Both);
        let _ = tunnel_stream.shutdown(Shutdown::Both);
        return Err(RelayTcpError::Io(err));
    }

    let status_byte = status_buf.first().copied().unwrap_or(0x01);
    if let Err(err) = session.complete_tcp_open(status_byte) {
        let _ = guest_stream.shutdown(Shutdown::Both);
        let _ = tunnel_stream.shutdown(Shutdown::Both);
        return Err(err);
    }

    let copy_result = bridge_tcp_bidirectional(guest_stream, tunnel_stream);
    match copy_result {
        Ok((g2e, e2g)) => {
            session.finish_tcp_flow(g2e, e2g);
            Ok((g2e, e2g))
        }
        Err(err) => {
            session.finish_tcp_flow(0, 0);
            Err(RelayTcpError::Io(err))
        }
    }
}

fn configure_stream_timeouts(a: &TcpStream, b: &TcpStream, timeout: Duration) -> io::Result<()> {
    let clamped = if timeout.is_zero() {
        DEFAULT_TCP_IO_TIMEOUT
    } else {
        timeout
    };
    a.set_read_timeout(Some(clamped))?;
    a.set_write_timeout(Some(clamped))?;
    b.set_read_timeout(Some(clamped))?;
    b.set_write_timeout(Some(clamped))?;
    Ok(())
}

/// Bridge two [`TcpStream`]s bidirectionally using bounded `16 KiB` buffers and propagating
/// half-close (`EOF` -> `Shutdown::Write`) independently in each direction (`NET-001`).
pub fn bridge_tcp_bidirectional(left: TcpStream, right: TcpStream) -> io::Result<(u64, u64)> {
    let mut left_reader = left.try_clone()?;
    let mut right_writer = right.try_clone()?;
    let mut right_reader = right;
    let mut left_writer = left;

    let left_to_right = thread::spawn(move || -> io::Result<u64> {
        let bytes = pump_with_half_close(&mut left_reader, &mut right_writer)?;
        let _ = right_writer.shutdown(Shutdown::Write);
        Ok(bytes)
    });

    let right_to_left_res = (|| -> io::Result<u64> {
        let bytes = pump_with_half_close(&mut right_reader, &mut left_writer)?;
        let _ = left_writer.shutdown(Shutdown::Write);
        Ok(bytes)
    })();

    let left_to_right_res = match left_to_right.join() {
        Ok(res) => res,
        Err(_) => Err(io::Error::other(
            "TCP forwarding worker thread terminated unexpectedly",
        )),
    };

    Ok((left_to_right_res?, right_to_left_res?))
}

fn pump_with_half_close<R: Read, W: Write>(reader: &mut R, writer: &mut W) -> io::Result<u64> {
    let mut buf = [0u8; TCP_COPY_CHUNK_BYTES];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            writer.flush()?;
            return Ok(total);
        }
        if let Some(slice) = buf.get(..n) {
            writer.write_all(slice)?;
            total = total.saturating_add(u64::try_from(n).unwrap_or(0));
        }
    }
}
