//! Engine-side TCP forwarding path (`T-061`, `contracts.md` §§11, 13, `NET-001`, `NET-003`, `SEC-006`).
//!
//! ## Wire protocol (`contracts.md` §11)
//!
//! 1. Over the engine's outbound-initiated tunnel session (`NET-003`), the relay opens a stream and
//!    sends a 3-byte [`TcpOpenHeader`] (`[0x01, mapping_index_hi, mapping_index_lo]`).
//! 2. The engine looks `mapping_index` up in its **own local mapping configuration** (`SEC-006`):
//!    * If `active_tcp_flows >= max_tcp_flows`, the engine writes `0x02`
//!      ([`TcpOpenStatus::LimitReached`]) and closes the stream.
//!    * If `mapping_index` is unknown, disabled (`!enabled`), or not `Protocol::Tcp`, the engine
//!      writes `0x01` ([`TcpOpenStatus::Refused`]) and closes the stream.
//! 3. The engine dials the configured local game server (`TcpStream::connect_timeout`):
//!    * On dial failure, the engine writes `0x01` ([`TcpOpenStatus::Refused`]) and closes the
//!      stream.
//!    * On dial success, the engine writes `0x00` ([`TcpOpenStatus::Connected`]) and forwards bytes
//!      bidirectionally with `16 KiB` buffers, honoring half-close (`EOF` -> `Shutdown::Write`) in
//!      each direction (`NET-001`).

use std::collections::HashMap;
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::thread;
use std::time::Duration;

use gtt_protocol::{
    is_valid_identifier, FrameError, Protocol, TcpOpenHeader, TcpOpenStatus, TCP_OPEN_HEADER_LEN,
};

/// Bounded buffer size used per directional TCP copy loop (`16 KiB`, `AGENTS.md` §4).
pub const ENGINE_TCP_CHUNK_BYTES: usize = 16_384;

/// Default timeout when dialing the local game server (`3 seconds`, `AGENTS.md` §4).
pub const DEFAULT_LOCAL_DIAL_TIMEOUT: Duration = Duration::from_secs(3);

/// Default read/write timeout on forwarded TCP streams (`30 seconds`, `AGENTS.md` §4).
pub const DEFAULT_ENGINE_TCP_IO_TIMEOUT: Duration = Duration::from_secs(30);

/// Host-local target configuration for a single profile mapping (`contracts.md` §§11, 13, `SEC-006`).
///
/// This structure is strictly local to the host engine and is never serialized to or accepted from
/// the control plane or relay (`INV-1`, `ADR-003`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalMappingConfig {
    /// Mapping index (`0..=7`) matching `hello_ack.mappings`.
    pub index: u16,
    /// Mapping key (`^[a-z][a-z0-9-]{0,31}$`).
    pub mapping_key: String,
    /// Transport protocol (`Tcp` or `Udp`).
    pub protocol: Protocol,
    /// Whether the host has enabled this mapping (`GAME-008`: optional mappings default off).
    pub enabled: bool,
    /// Host-local target socket address (e.g. `127.0.0.1:25565`).
    pub local_addr: SocketAddr,
    /// Timeout when dialing the local target (`AGENTS.md` §4).
    pub connect_timeout: Duration,
    /// Read/write timeout on forwarded streams (`AGENTS.md` §4).
    pub io_timeout: Duration,
}

impl LocalMappingConfig {
    /// Construct a new enabled [`LocalMappingConfig`] with standard timeouts.
    pub fn new(
        index: u16,
        mapping_key: impl Into<String>,
        protocol: Protocol,
        enabled: bool,
        local_addr: SocketAddr,
    ) -> Self {
        Self {
            index,
            mapping_key: mapping_key.into(),
            protocol,
            enabled,
            local_addr,
            connect_timeout: DEFAULT_LOCAL_DIAL_TIMEOUT,
            io_timeout: DEFAULT_ENGINE_TCP_IO_TIMEOUT,
        }
    }

    /// Validate basic structural bounds on a local mapping entry.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.index > 7 {
            return Err("mapping index must be 0..=7");
        }
        if !is_valid_identifier(&self.mapping_key) {
            return Err("mapping_key must match ^[a-z][a-z0-9-]{0,31}$");
        }
        if self.local_addr.port() == 0 {
            return Err("local_addr port must be 1..=65535");
        }
        if self.local_addr.ip().is_unspecified() || self.local_addr.ip().is_multicast() {
            return Err("local_addr IP must not be unspecified or multicast (contracts.md §13)");
        }
        if self.connect_timeout.is_zero() || self.connect_timeout > Duration::from_secs(30) {
            return Err("connect_timeout must be between 1ms and 30s");
        }
        if self.io_timeout.is_zero() || self.io_timeout > Duration::from_secs(300) {
            return Err("io_timeout must be between 1ms and 300s");
        }
        Ok(())
    }
}

/// Metrics tracked by the engine TCP forwarder (`T-061`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EngineTcpMetrics {
    /// Currently active TCP streams dialed to local targets.
    pub active_tcp_flows: u16,
    /// Total streams that dialed the local target and returned `0x00` (`Connected`).
    pub total_connected: u64,
    /// Total streams refused with `0x01` (`Refused`: unknown/disabled mapping or local dial error).
    pub total_refused: u64,
    /// Total streams refused with `0x02` (`LimitReached`: `max_tcp_flows` reached).
    pub total_limit_reached: u64,
    /// Total bytes forwarded from the relay tunnel stream to the local game server.
    pub bytes_to_local: u64,
    /// Total bytes forwarded from the local game server to the relay tunnel stream.
    pub bytes_from_local: u64,
}

/// Errors returned by the engine TCP forwarder.
#[derive(Debug)]
pub enum EngineTcpError {
    /// Malformed 3-byte [`TcpOpenHeader`] (`NET-007`).
    Protocol(FrameError),
    /// Unknown, disabled, or non-TCP `mapping_index` (`SEC-006`).
    RefusedMapping {
        /// Mapping index requested by the relay.
        mapping_index: u16,
    },
    /// Session `max_tcp_flows` reached (`0x02`).
    LimitReached {
        /// Configured `max_tcp_flows`.
        max_tcp_flows: u16,
    },
    /// Failed to dial the configured local target (`0x01`).
    LocalDialFailed {
        /// Mapping index that failed to connect.
        mapping_index: u16,
        /// Underlying socket error.
        source: io::Error,
    },
    /// Stream I/O failure while reading header, writing status, or copying bytes.
    Io(io::Error),
}

impl fmt::Display for EngineTcpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protocol(err) => write!(f, "invalid TCP open header: {err}"),
            Self::RefusedMapping { mapping_index } => {
                write!(
                    f,
                    "refused unknown, disabled, or non-TCP mapping_index {mapping_index}"
                )
            }
            Self::LimitReached { max_tcp_flows } => {
                write!(f, "engine TCP flow limit ({max_tcp_flows}) reached")
            }
            Self::LocalDialFailed {
                mapping_index,
                source,
            } => write!(
                f,
                "failed to dial local target for mapping_index {mapping_index}: {source}"
            ),
            Self::Io(err) => write!(f, "engine TCP stream I/O error: {err}"),
        }
    }
}

impl std::error::Error for EngineTcpError {}

/// Engine-side TCP session manager and local target dialer (`T-061`, `NET-001`, `SEC-006`).
#[derive(Debug, Clone)]
pub struct EngineTcpSession {
    mappings: HashMap<u16, LocalMappingConfig>,
    max_tcp_flows: u16,
    metrics: EngineTcpMetrics,
}

impl EngineTcpSession {
    /// Construct a validated [`EngineTcpSession`] from the host's local mapping configuration.
    pub fn new(configs: Vec<LocalMappingConfig>, max_tcp_flows: u16) -> Result<Self, &'static str> {
        let mut mappings = HashMap::with_capacity(configs.len());
        for cfg in configs {
            cfg.validate()?;
            if mappings.insert(cfg.index, cfg).is_some() {
                return Err("duplicate mapping index in LocalMappingConfig list");
            }
        }
        Ok(Self {
            mappings,
            max_tcp_flows: max_tcp_flows.clamp(1, 4096),
            metrics: EngineTcpMetrics::default(),
        })
    }

    /// Borrow current TCP forwarding metrics.
    pub fn metrics(&self) -> &EngineTcpMetrics {
        &self.metrics
    }

    /// Evaluate a 3-byte [`TcpOpenHeader`] against the engine's local mapping table and flow cap.
    ///
    /// On success, reserves a flow slot and returns the [`LocalMappingConfig`] to dial.
    /// On failure, returns the wire [`TcpOpenStatus`] byte (`0x01` or `0x02`) to send back before
    /// closing the stream (`contracts.md` §11, `SEC-006`).
    pub fn evaluate_open_header(
        &mut self,
        header_bytes: &[u8],
    ) -> Result<LocalMappingConfig, (TcpOpenStatus, EngineTcpError)> {
        let header = match TcpOpenHeader::decode(header_bytes) {
            Ok(h) => h,
            Err(err) => {
                self.metrics.total_refused = self.metrics.total_refused.saturating_add(1);
                return Err((TcpOpenStatus::Refused, EngineTcpError::Protocol(err)));
            }
        };

        let Some(cfg) = self.mappings.get(&header.mapping_index) else {
            self.metrics.total_refused = self.metrics.total_refused.saturating_add(1);
            return Err((
                TcpOpenStatus::Refused,
                EngineTcpError::RefusedMapping {
                    mapping_index: header.mapping_index,
                },
            ));
        };

        if !cfg.enabled || cfg.protocol != Protocol::Tcp {
            self.metrics.total_refused = self.metrics.total_refused.saturating_add(1);
            return Err((
                TcpOpenStatus::Refused,
                EngineTcpError::RefusedMapping {
                    mapping_index: header.mapping_index,
                },
            ));
        }

        if self.metrics.active_tcp_flows >= self.max_tcp_flows {
            self.metrics.total_limit_reached = self.metrics.total_limit_reached.saturating_add(1);
            return Err((
                TcpOpenStatus::LimitReached,
                EngineTcpError::LimitReached {
                    max_tcp_flows: self.max_tcp_flows,
                },
            ));
        }

        self.metrics.active_tcp_flows = self.metrics.active_tcp_flows.saturating_add(1);
        Ok(cfg.clone())
    }

    /// Record that a local dial attempt failed after [`Self::evaluate_open_header`] reserved a slot.
    pub fn record_local_dial_failure(&mut self) {
        self.metrics.active_tcp_flows = self.metrics.active_tcp_flows.saturating_sub(1);
        self.metrics.total_refused = self.metrics.total_refused.saturating_add(1);
    }

    /// Record that the local dial succeeded (`0x00` sent).
    pub fn record_local_dial_connected(&mut self) {
        self.metrics.total_connected = self.metrics.total_connected.saturating_add(1);
    }

    /// Release an active flow slot and accumulate transferred byte counts.
    pub fn finish_tcp_flow(&mut self, bytes_to_local: u64, bytes_from_local: u64) {
        self.metrics.active_tcp_flows = self.metrics.active_tcp_flows.saturating_sub(1);
        self.metrics.bytes_to_local = self.metrics.bytes_to_local.saturating_add(bytes_to_local);
        self.metrics.bytes_from_local = self
            .metrics
            .bytes_from_local
            .saturating_add(bytes_from_local);
    }

    /// Reset all active TCP flows when the tunnel session drops (`contracts.md` §10).
    pub fn close_all_flows_on_session_drop(&mut self) -> u16 {
        let closed = self.metrics.active_tcp_flows;
        self.metrics.active_tcp_flows = 0;
        closed
    }
}

/// Accept a relay-opened TCP stream on the engine side: read the 3-byte [`TcpOpenHeader`],
/// validate the mapping in `session`, dial the local target, write the 1-byte [`TcpOpenStatus`]
/// reply (`0x00`/`0x01`/`0x02`), and forward bytes bidirectionally with half-close (`NET-001`,
/// `SEC-006`).
///
/// Returns `(bytes_to_local, bytes_from_local)` on a completed `0x00` stream.
pub fn engine_handle_tcp_stream(
    session: &mut EngineTcpSession,
    mut tunnel_stream: TcpStream,
) -> Result<(u64, u64), EngineTcpError> {
    let _ = tunnel_stream.set_read_timeout(Some(DEFAULT_ENGINE_TCP_IO_TIMEOUT));
    let _ = tunnel_stream.set_write_timeout(Some(DEFAULT_ENGINE_TCP_IO_TIMEOUT));

    let mut header_buf = [0u8; TCP_OPEN_HEADER_LEN];
    if let Err(err) = tunnel_stream.read_exact(&mut header_buf) {
        let _ = tunnel_stream.shutdown(Shutdown::Both);
        return Err(EngineTcpError::Io(err));
    }

    let cfg = match session.evaluate_open_header(&header_buf) {
        Ok(cfg) => cfg,
        Err((status, err)) => {
            let _ = tunnel_stream.write_all(&[status.to_byte()]);
            let _ = tunnel_stream.flush();
            let _ = tunnel_stream.shutdown(Shutdown::Both);
            return Err(err);
        }
    };

    let local_stream = match TcpStream::connect_timeout(&cfg.local_addr, cfg.connect_timeout) {
        Ok(stream) => stream,
        Err(dial_err) => {
            session.record_local_dial_failure();
            let _ = tunnel_stream.write_all(&[TcpOpenStatus::Refused.to_byte()]);
            let _ = tunnel_stream.flush();
            let _ = tunnel_stream.shutdown(Shutdown::Both);
            return Err(EngineTcpError::LocalDialFailed {
                mapping_index: cfg.index,
                source: dial_err,
            });
        }
    };

    let _ = local_stream.set_read_timeout(Some(cfg.io_timeout));
    let _ = local_stream.set_write_timeout(Some(cfg.io_timeout));
    let _ = tunnel_stream.set_read_timeout(Some(cfg.io_timeout));
    let _ = tunnel_stream.set_write_timeout(Some(cfg.io_timeout));

    if let Err(err) = tunnel_stream
        .write_all(&[TcpOpenStatus::Connected.to_byte()])
        .and_then(|()| tunnel_stream.flush())
    {
        session.record_local_dial_failure();
        let _ = local_stream.shutdown(Shutdown::Both);
        let _ = tunnel_stream.shutdown(Shutdown::Both);
        return Err(EngineTcpError::Io(err));
    }

    session.record_local_dial_connected();

    match bridge_engine_tcp_streams(tunnel_stream, local_stream) {
        Ok((to_local, from_local)) => {
            session.finish_tcp_flow(to_local, from_local);
            Ok((to_local, from_local))
        }
        Err(err) => {
            session.finish_tcp_flow(0, 0);
            Err(EngineTcpError::Io(err))
        }
    }
}

/// Bridge `tunnel_stream` and `local_stream` bidirectionally with `16 KiB` buffers and independent
/// half-close (`EOF` -> `Shutdown::Write`, `NET-001`).
pub fn bridge_engine_tcp_streams(
    tunnel_stream: TcpStream,
    local_stream: TcpStream,
) -> io::Result<(u64, u64)> {
    let mut tunnel_reader = tunnel_stream.try_clone()?;
    let mut local_writer = local_stream.try_clone()?;
    let mut local_reader = local_stream;
    let mut tunnel_writer = tunnel_stream;

    let to_local_handle = thread::spawn(move || -> io::Result<u64> {
        let bytes = pump_direction(&mut tunnel_reader, &mut local_writer)?;
        let _ = local_writer.shutdown(Shutdown::Write);
        Ok(bytes)
    });

    let from_local_res = (|| -> io::Result<u64> {
        let bytes = pump_direction(&mut local_reader, &mut tunnel_writer)?;
        let _ = tunnel_writer.shutdown(Shutdown::Write);
        Ok(bytes)
    })();

    let to_local_res = match to_local_handle.join() {
        Ok(res) => res,
        Err(_) => Err(io::Error::other(
            "engine TCP worker thread terminated unexpectedly",
        )),
    };

    Ok((to_local_res?, from_local_res?))
}

fn pump_direction<R: Read, W: Write>(reader: &mut R, writer: &mut W) -> io::Result<u64> {
    let mut buf = [0u8; ENGINE_TCP_CHUNK_BYTES];
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
